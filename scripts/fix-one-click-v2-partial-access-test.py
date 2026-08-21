from pathlib import Path

p = Path("apps/desktop/src/MilestoneOneClick.test.tsx")
s = p.read_text()

old = '    expect(api.registerUserContentRoot).toHaveBeenCalledWith("workspace-1", "desktop");\n'
if old in s:
    s = s.replace(
        old,
        '    expect(api.buildOneClickV2Plan).toHaveBeenCalledTimes(1);\n',
        1,
    )

old = '''    expect(api.registerUserContentRoot).not.toHaveBeenCalledWith(
      "workspace-1",
      "pictures",
    );'''
if old in s:
    s = s.replace(
        old,
        '    expect(api.registerUserContentRoot).not.toHaveBeenCalled();',
        1,
    )

p.write_text(s)
print("remaining one-click v2 partial-access assertions migrated")
