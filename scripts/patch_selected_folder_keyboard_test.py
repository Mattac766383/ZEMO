from pathlib import Path
import re

path = Path(__file__).resolve().parents[1] / "apps/desktop/src/Milestone12Walkthrough.test.tsx"
text = path.read_text(encoding="utf-8")
pattern = re.compile(
    r'  it\("keeps the essential keyboard flow reachable", async \(\) => \{.*?\n  \}\);',
    re.S,
)
replacement = '''  it("keeps the essential keyboard flow reachable", async () => {
    render(<App />);
    const dialog = await screen.findByRole("dialog");
    const primary = within(dialog).getByRole("button", { name: "Continuer" });
    primary.focus();
    expect(document.activeElement).toBe(primary);

    fireEvent.click(primary);
    const second = within(screen.getByRole("dialog")).getByRole("button", { name: "Continuer" });
    second.focus();
    expect(document.activeElement).toBe(second);
    fireEvent.click(second);

    const choose = within(screen.getByRole("dialog")).getByRole("button", {
      name: "Choisir un dossier à ranger",
    });
    expect((choose as HTMLButtonElement).disabled).toBe(false);
    expect(choose.getAttribute("type")).toBe("button");
    choose.focus();
    expect(document.activeElement).toBe(choose);
    expect(window.innerWidth).toBe(1280);
    expect(window.innerHeight).toBe(800);
  });'''
text, count = pattern.subn(replacement, text, count=1)
if count != 1:
    raise RuntimeError(f"keyboard test: expected 1 match, got {count}")
path.write_text(text, encoding="utf-8")
print("selected-folder keyboard test patched")
