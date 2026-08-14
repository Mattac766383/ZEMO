use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct OcrRequest<'a> {
    pub image_bytes: &'a [u8],
    pub media_type: &'a str,
    pub languages: &'a [String],
    pub max_output_characters: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrResult {
    pub text: String,
    pub mean_confidence: Option<f32>,
    pub blocks: Vec<OcrBlock>,
    pub engine_version: String,
    pub language_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrBlock {
    pub text: String,
    pub normalized_box: [f32; 4],
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrErrorKind {
    Unavailable,
    Cancelled,
    Timeout,
    InvalidInput,
    ProcessFailed,
    OutputTooLarge,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct OcrError {
    pub kind: OcrErrorKind,
    pub message: String,
}

impl OcrError {
    fn new(kind: OcrErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// OCR providers are deliberately local-only. The extraction crate has no
/// network client and offers no cloud-provider or fallback variant.
pub trait OcrProvider: Send + Sync {
    fn recognize(
        &self,
        request: &OcrRequest<'_>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<OcrResult, OcrError>;

    fn provider_name(&self) -> &'static str;
}

#[derive(Debug, Default)]
pub struct UnavailableOcrProvider;

impl OcrProvider for UnavailableOcrProvider {
    fn recognize(
        &self,
        _request: &OcrRequest<'_>,
        _is_cancelled: &dyn Fn() -> bool,
    ) -> Result<OcrResult, OcrError> {
        Err(OcrError::new(
            OcrErrorKind::Unavailable,
            "no trusted local OCR engine is installed",
        ))
    }

    fn provider_name(&self) -> &'static str {
        "unavailable"
    }
}

#[derive(Debug, Clone)]
pub struct TesseractOcrProvider {
    executable: PathBuf,
    timeout: Duration,
    max_process_output_bytes: usize,
}

impl TesseractOcrProvider {
    pub fn new(executable: impl Into<PathBuf>, timeout: Duration) -> Result<Self, OcrError> {
        let executable = executable.into();
        if !executable.is_absolute() {
            return Err(OcrError::new(
                OcrErrorKind::Unavailable,
                "the local OCR executable path must be absolute",
            ));
        }
        Ok(Self {
            executable,
            timeout,
            max_process_output_bytes: 16 * 1024 * 1024,
        })
    }

    #[must_use]
    pub fn auto_detect(timeout: Duration) -> Option<Self> {
        trusted_tesseract_paths()
            .into_iter()
            .find(|path| path.is_file())
            .and_then(|path| Self::new(path, timeout).ok())
    }
}

impl OcrProvider for TesseractOcrProvider {
    fn recognize(
        &self,
        request: &OcrRequest<'_>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<OcrResult, OcrError> {
        if !matches!(
            request.media_type,
            "image/png" | "image/jpeg" | "image/webp" | "image/tiff" | "image/bmp"
        ) {
            return Err(OcrError::new(
                OcrErrorKind::InvalidInput,
                "the local OCR provider does not accept this image type",
            ));
        }
        if is_cancelled() {
            return Err(OcrError::new(OcrErrorKind::Cancelled, "OCR was cancelled"));
        }

        let mut command = Command::new(&self.executable);
        command.env_clear().args(["stdin", "stdout"]);
        if !request.languages.is_empty() {
            command.arg("-l").arg(request.languages.join("+"));
        }
        command.arg("tsv");
        let output = run_local_process(
            command,
            request.image_bytes,
            self.timeout,
            self.max_process_output_bytes,
            is_cancelled,
        )?;
        parse_tesseract_tsv(
            &output,
            request.image_bytes,
            request.max_output_characters,
            request.languages.first().cloned(),
        )
    }

    fn provider_name(&self) -> &'static str {
        "tesseract_local"
    }
}

pub trait PdfPageRenderer: Send + Sync {
    fn render_page(
        &self,
        pdf_bytes: &[u8],
        page_number: u32,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, OcrError>;

    fn renderer_name(&self) -> &'static str;
}

#[derive(Debug, Clone)]
pub struct PdftoppmRenderer {
    executable: PathBuf,
    timeout: Duration,
    max_output_bytes: usize,
}

impl PdftoppmRenderer {
    pub fn new(executable: impl Into<PathBuf>, timeout: Duration) -> Result<Self, OcrError> {
        let executable = executable.into();
        if !executable.is_absolute() {
            return Err(OcrError::new(
                OcrErrorKind::Unavailable,
                "the local PDF renderer path must be absolute",
            ));
        }
        Ok(Self {
            executable,
            timeout,
            max_output_bytes: 48 * 1024 * 1024,
        })
    }

    #[must_use]
    pub fn auto_detect(timeout: Duration) -> Option<Self> {
        trusted_pdftoppm_paths()
            .into_iter()
            .find(|path| path.is_file())
            .and_then(|path| Self::new(path, timeout).ok())
    }
}

impl PdfPageRenderer for PdftoppmRenderer {
    fn render_page(
        &self,
        pdf_bytes: &[u8],
        page_number: u32,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, OcrError> {
        let page = page_number.to_string();
        let mut command = Command::new(&self.executable);
        command.env_clear().args([
            "-f",
            &page,
            "-l",
            &page,
            "-singlefile",
            "-png",
            "-scale-to",
            "3000",
            "-",
            "-",
        ]);
        run_local_process(
            command,
            pdf_bytes,
            self.timeout,
            self.max_output_bytes,
            is_cancelled,
        )
    }

    fn renderer_name(&self) -> &'static str {
        "pdftoppm_local"
    }
}

fn run_local_process(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    max_output_bytes: usize,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, OcrError> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            OcrError::new(
                OcrErrorKind::Unavailable,
                format!("local extraction process is unavailable: {error}"),
            )
        })?;

    let stdout = child.stdout.take().ok_or_else(|| {
        OcrError::new(
            OcrErrorKind::ProcessFailed,
            "local extraction process has no output stream",
        )
    })?;
    let output_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(u64::try_from(max_output_bytes).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut output)
            .map(|_| output)
    });

    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| {
            OcrError::new(
                OcrErrorKind::ProcessFailed,
                "local extraction process has no input stream",
            )
        })
        .and_then(|mut stdin| {
            stdin.write_all(input).map_err(|error| {
                OcrError::new(
                    OcrErrorKind::ProcessFailed,
                    format!("could not pass bounded input to local process: {error}"),
                )
            })
        });
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        let _ = output_reader.join();
        return Err(error);
    }

    let started = Instant::now();
    let status = loop {
        if is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = output_reader.join();
            return Err(OcrError::new(
                OcrErrorKind::Cancelled,
                "local extraction process was cancelled",
            ));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = output_reader.join();
            return Err(OcrError::new(
                OcrErrorKind::Timeout,
                "local extraction process exceeded its time limit",
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = output_reader.join();
                return Err(OcrError::new(
                    OcrErrorKind::ProcessFailed,
                    format!("could not monitor local extraction process: {error}"),
                ));
            }
        }
    };
    let output = output_reader
        .join()
        .map_err(|_| {
            OcrError::new(
                OcrErrorKind::ProcessFailed,
                "local extraction output reader stopped unexpectedly",
            )
        })?
        .map_err(|error| {
            OcrError::new(
                OcrErrorKind::ProcessFailed,
                format!("could not read local extraction output: {error}"),
            )
        })?;
    if output.len() > max_output_bytes {
        return Err(OcrError::new(
            OcrErrorKind::OutputTooLarge,
            "local extraction output exceeded its safety limit",
        ));
    }
    if !status.success() {
        return Err(OcrError::new(
            OcrErrorKind::ProcessFailed,
            "local extraction process rejected the document",
        ));
    }
    Ok(output)
}

fn parse_tesseract_tsv(
    bytes: &[u8],
    image_bytes: &[u8],
    max_output_characters: usize,
    language_hint: Option<String>,
) -> Result<OcrResult, OcrError> {
    let tsv = std::str::from_utf8(bytes).map_err(|_| {
        OcrError::new(
            OcrErrorKind::ProcessFailed,
            "local OCR returned invalid UTF-8",
        )
    })?;
    let dimensions = imagesize::blob_size(image_bytes).ok();
    let image_width = dimensions.as_ref().map_or(1_f32, |value| {
        u32::try_from(value.width).unwrap_or(u32::MAX) as f32
    });
    let image_height = dimensions.as_ref().map_or(1_f32, |value| {
        u32::try_from(value.height).unwrap_or(u32::MAX) as f32
    });
    let mut text = String::new();
    let mut blocks = Vec::new();
    let mut confidence_sum = 0_f32;
    let mut confidence_count = 0_u32;
    let mut previous_line = None::<(u32, u32, u32, u32)>;

    for row in tsv.lines().skip(1) {
        let columns = row.splitn(12, '\t').collect::<Vec<_>>();
        if columns.len() != 12 || columns[0] != "5" {
            continue;
        }
        let word = columns[11].trim();
        if word.is_empty() {
            continue;
        }
        let line = (
            parse_u32(columns[1]),
            parse_u32(columns[2]),
            parse_u32(columns[3]),
            parse_u32(columns[4]),
        );
        if previous_line.is_some() && previous_line != Some(line) {
            text.push('\n');
        } else if !text.is_empty() {
            text.push(' ');
        }
        previous_line = Some(line);
        text.push_str(word);

        let confidence = columns[10]
            .parse::<f32>()
            .ok()
            .filter(|value| *value >= 0.0)
            .map(|value| (value / 100.0).clamp(0.0, 1.0));
        if let Some(value) = confidence {
            confidence_sum += value;
            confidence_count = confidence_count.saturating_add(1);
        }
        let left = parse_u32(columns[6]) as f32 / image_width;
        let top = parse_u32(columns[7]) as f32 / image_height;
        let width = parse_u32(columns[8]) as f32 / image_width;
        let height = parse_u32(columns[9]) as f32 / image_height;
        blocks.push(OcrBlock {
            text: word.to_owned(),
            normalized_box: [
                left.clamp(0.0, 1.0),
                top.clamp(0.0, 1.0),
                width.clamp(0.0, 1.0),
                height.clamp(0.0, 1.0),
            ],
            confidence,
        });
    }
    if text.chars().count() > max_output_characters {
        text = text.chars().take(max_output_characters).collect();
    }
    Ok(OcrResult {
        text,
        mean_confidence: (confidence_count > 0).then(|| confidence_sum / confidence_count as f32),
        blocks,
        engine_version: "tesseract-cli".to_owned(),
        language_hint,
    })
}

fn parse_u32(value: &str) -> u32 {
    value.parse().unwrap_or(0)
}

fn trusted_tesseract_paths() -> Vec<PathBuf> {
    let mut paths = bundled_sidecar_paths("tesseract");
    #[cfg(target_os = "macos")]
    {
        paths.extend([
            PathBuf::from("/opt/homebrew/bin/tesseract"),
            PathBuf::from("/usr/local/bin/tesseract"),
            PathBuf::from("/usr/bin/tesseract"),
        ]);
    }
    #[cfg(target_os = "windows")]
    {
        paths.push(PathBuf::from(
            r"C:\Program Files\Tesseract-OCR\tesseract.exe",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        paths.extend([
            PathBuf::from("/usr/bin/tesseract"),
            PathBuf::from("/usr/local/bin/tesseract"),
        ]);
    }
    if let Some(path) = std::env::var_os("SUPREMACY_TESSERACT_PATH") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            paths.insert(0, path);
        }
    }
    paths
}

fn trusted_pdftoppm_paths() -> Vec<PathBuf> {
    let mut paths = bundled_sidecar_paths("pdftoppm");
    #[cfg(target_os = "macos")]
    {
        paths.extend([
            PathBuf::from("/opt/homebrew/bin/pdftoppm"),
            PathBuf::from("/usr/local/bin/pdftoppm"),
            PathBuf::from("/usr/bin/pdftoppm"),
        ]);
    }
    #[cfg(target_os = "windows")]
    {
        paths.push(PathBuf::from(
            r"C:\Program Files\poppler\Library\bin\pdftoppm.exe",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        paths.extend([
            PathBuf::from("/usr/bin/pdftoppm"),
            PathBuf::from("/usr/local/bin/pdftoppm"),
        ]);
    }
    if let Some(path) = std::env::var_os("SUPREMACY_PDFTOPPM_PATH") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            paths.insert(0, path);
        }
    }
    paths
}

/// Production packaging discovery order for local OCR/PDF renderers:
/// 1. absolute env override (`SUPREMACY_TESSERACT_PATH` / `SUPREMACY_PDFTOPPM_PATH`)
/// 2. app-bundled sidecars next to the current executable / `resources/`
/// 3. trusted host absolute paths (Homebrew / Program Files / /usr/bin)
///
/// Relative PATH lookups are intentionally rejected.
fn bundled_sidecar_paths(binary_name: &str) -> Vec<PathBuf> {
    let file_name = if cfg!(target_os = "windows") {
        format!("{binary_name}.exe")
    } else {
        binary_name.to_owned()
    };
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join(&file_name));
        paths.push(dir.join("resources").join(&file_name));
        paths.push(dir.join("sidecars").join(&file_name));
        // macOS .app layout: Contents/MacOS → Contents/Resources
        if let Some(contents) = dir.parent() {
            paths.push(contents.join("Resources").join(&file_name));
            paths.push(contents.join("Resources").join("sidecars").join(&file_name));
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_parser_preserves_lines_and_confidence() {
        let tsv = b"level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t90\tHello\n\
5\t1\t1\t1\t1\t2\t12\t0\t10\t10\t80\tworld\n\
5\t1\t1\t1\t2\t1\t0\t12\t10\t10\t70\tAgain\n";
        let result = parse_tesseract_tsv(tsv, &[], 1_000, Some("eng".to_owned()))
            .expect("synthetic local OCR output should parse");
        assert_eq!(result.text, "Hello world\nAgain");
        assert_eq!(result.language_hint.as_deref(), Some("eng"));
        assert!(result.mean_confidence.is_some_and(|value| value > 0.79));
    }

    #[test]
    fn provider_rejects_relative_executable_paths() {
        assert!(TesseractOcrProvider::new("tesseract", Duration::from_secs(1)).is_err());
    }
}
