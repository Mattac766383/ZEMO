use extraction::{DeterministicExtractor, ExtractedDocument, ExtractionEngine, ExtractionRequest};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

const MAX_REQUEST_BYTES: usize = 300 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct WorkerRequest {
    protocol_version: u32,
    extraction: ExtractionRequest,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WorkerResponse {
    Success {
        protocol_version: u32,
        document: ExtractedDocument,
    },
    Error {
        protocol_version: u32,
        request_id: Option<String>,
        code: String,
    },
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let response = match line {
            Ok(line) if line.len() <= MAX_REQUEST_BYTES => handle(&line),
            Ok(_) => WorkerResponse::Error {
                protocol_version: 1,
                request_id: None,
                code: "request_too_large".to_owned(),
            },
            Err(_) => WorkerResponse::Error {
                protocol_version: 1,
                request_id: None,
                code: "stdin_error".to_owned(),
            },
        };
        if serde_json::to_writer(&mut stdout, &response).is_err() {
            break;
        }
        if stdout.write_all(b"\n").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}

fn handle(line: &str) -> WorkerResponse {
    let request = match serde_json::from_str::<WorkerRequest>(line) {
        Ok(request) => request,
        Err(_) => {
            return WorkerResponse::Error {
                protocol_version: 1,
                request_id: None,
                code: "invalid_request".to_owned(),
            };
        }
    };
    if request.protocol_version != 1 {
        return WorkerResponse::Error {
            protocol_version: 1,
            request_id: Some(request.extraction.request_id),
            code: "unsupported_protocol".to_owned(),
        };
    }
    let request_id = request.extraction.request_id.clone();
    match DeterministicExtractor.extract(&request.extraction) {
        Ok(document) => WorkerResponse::Success {
            protocol_version: 1,
            document,
        },
        Err(_) => WorkerResponse::Error {
            protocol_version: 1,
            request_id: Some(request_id),
            code: "extraction_failed".to_owned(),
        },
    }
}
