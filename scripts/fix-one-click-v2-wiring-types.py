from pathlib import Path

app = Path("apps/desktop/src/App.tsx")
s = app.read_text()
s = s.replace("  generateOrganizationProposal,\n", "")
head = s[:4000]
if "  OneClickV2Plan,\n" not in head:
    anchor = "  MonitoringDashboard,\n"
    if anchor not in s:
        raise SystemExit("OneClickV2Plan import anchor missing")
    s = s.replace(anchor, anchor + "  OneClickV2Plan,\n", 1)
app.write_text(s)
print("one-click v2 frontend import fixes applied")
