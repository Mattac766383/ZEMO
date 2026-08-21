from pathlib import Path
import subprocess

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

# Keep the migration self-contained for the existing CI repair job: it already
# invokes this script after applying the product wiring. The old tests asserted
# that the legacy register->scan->proposal pipeline was called; v2 intentionally
# removes those calls, so migrate those contracts before running Vitest.
subprocess.run(["python3", "scripts/update-one-click-v2-tests.py"], check=True)
# The repair job stages product files explicitly. Stage migrated tests here so
# the successful bot commit contains the verified contracts too.
subprocess.run(
    [
        "git",
        "add",
        "apps/desktop/src/MilestoneOneClick.test.tsx",
        "apps/desktop/src/Milestone12_2Ux.test.tsx",
    ],
    check=True,
)
print("one-click v2 frontend import and test fixes applied")
