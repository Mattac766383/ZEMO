from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "apps/desktop/src"

replacements = {
    "shows Choisir un dossier à ranger and Choisir un dossier à ranger on first run":
        "shows Ranger mon ordinateur and Choisir les dossiers on first run",
    "shows a three-step first launch then Choisir un dossier à ranger":
        "shows a three-step first launch then Ranger mon ordinateur",
}

for path in ROOT.glob("*.test.tsx"):
    text = path.read_text(encoding="utf-8")
    for current, expected in replacements.items():
        text = text.replace(current, expected)
    path.write_text(text, encoding="utf-8")

print("selected-folder test names normalized")
