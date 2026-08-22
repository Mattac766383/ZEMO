from pathlib import Path

p = Path("crates/organizer/src/organization.rs")
s = p.read_text()
old = '''        assert_eq!(
            by_name["invoice.pdf"].proposed_destination,
            ["Documents", "Administratif"]
        );'''
new = '''        assert_eq!(
            by_name["invoice.pdf"].proposed_destination,
            ["Documents", "Administratif", "Factures"]
        );'''
if old in s:
    s = s.replace(old, new, 1)
elif '["Documents", "Administratif", "Factures"]' not in s:
    raise SystemExit("consumer organization invoice expectation marker missing")
p.write_text(s)
print("One-Click v3 compatibility tests synchronized")
