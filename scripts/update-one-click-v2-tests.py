from pathlib import Path


def must_replace(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, got {count}")
    return text.replace(old, new, 1)


def moves(prefix: str = "/Users/local/Desktop") -> str:
    items = [
        ("invoice.pdf", "Documents/Administratif", "document_admin"),
        ("holiday.jpg", "Images", "image"),
        ("video.mp4", "Vidéos", "video"),
        ("archive.zip", "Archives", "archive"),
        ("setup.dmg", "Installateurs", "installer"),
        ("unknown.xyz", "À vérifier", "unknown_loose_file"),
        ("notes.txt", "Documents/Personnel", "document"),
    ]
    return "[\n" + "\n".join(
        f'''        {{ source: "{prefix}/{name}", destination: "{prefix}/{category}/{name}", category: "{category}", reason: "{reason}" }},'''
        for name, category, reason in items
    ) + "\n      ]"

# --- MilestoneOneClick ---
p = Path("apps/desktop/src/MilestoneOneClick.test.tsx")
s = p.read_text()

s = must_replace(
    s,
    "  probeUserContentAccess: vi.fn(),\n  authorizeUserContentFolder: vi.fn(),",
    "  probeUserContentAccess: vi.fn(),\n  buildOneClickV2Plan: vi.fn(),\n  applyOneClickV2: vi.fn(),\n  undoOneClickV2: vi.fn(),\n  authorizeUserContentFolder: vi.fn(),",
    "oneclick mock functions",
)

anchor = '''    vi.mocked(api.probeUserContentAccess).mockResolvedValue(\n      locations.filter((item) => item.recommended).map((item) => toProbe(item)),\n    );'''
insert = anchor + f'''\n    vi.mocked(api.buildOneClickV2Plan).mockResolvedValue({{\n      planId: "v2-plan-1",\n      filesSeen: 8,\n      proposedMoves: 7,\n      roots: [{{\n        kind: "desktop",\n        displayLabel: "Bureau",\n        root: "/Users/local/Desktop",\n        filesSeen: 8,\n        skipped: 1,\n        errors: [],\n        proposedMoves: {moves()},\n      }}],\n    }});\n    vi.mocked(api.applyOneClickV2).mockResolvedValue({{\n      applied: Array.from({{ length: 7 }}, (_, index) => ({{\n        source: `/Users/local/Desktop/source-${{index}}`,\n        destination: `/Users/local/Desktop/Documents/destination-${{index}}`,\n      }})),\n      skipped: 0,\n      errors: [],\n    }});\n    vi.mocked(api.undoOneClickV2).mockResolvedValue({{\n      applied: Array.from({{ length: 7 }}, (_, index) => ({{\n        source: `/Users/local/Desktop/Documents/destination-${{index}}`,\n        destination: `/Users/local/Desktop/source-${{index}}`,\n      }})),\n      skipped: 0,\n      errors: [],\n    }});'''
s = must_replace(s, anchor, insert, "oneclick beforeEach v2 mocks")

old = '''    await waitFor(() => {\n      expect(api.probeUserContentAccess).toHaveBeenCalled();\n      expect(api.registerUserContentRoot).toHaveBeenCalled();\n      expect(api.scanWorkspace).toHaveBeenCalled();\n      expect(api.generateOrganizationProposal).toHaveBeenCalled();\n    });\n    const generateArgs = vi.mocked(api.generateOrganizationProposal).mock.calls[0];\n    expect(generateArgs[3]).toBe(true);'''
new = '''    await waitFor(() => {\n      expect(api.probeUserContentAccess).toHaveBeenCalled();\n      expect(api.buildOneClickV2Plan).toHaveBeenCalledTimes(1);\n    });\n    expect(api.registerUserContentRoot).not.toHaveBeenCalled();\n    expect(api.scanWorkspace).not.toHaveBeenCalled();\n    expect(api.generateOrganizationProposal).not.toHaveBeenCalled();'''
s = must_replace(s, old, new, "oneclick main journey planning assertions")

old = '''    fireEvent.click(screen.getByRole("button", { name: "Annuler le rangement" }));\n    await waitFor(() => {\n      expect(api.rollbackExecution).toHaveBeenCalledWith("exec-1");\n    });'''
new = '''    await waitFor(() => {\n      expect(api.applyOneClickV2).toHaveBeenCalledTimes(1);\n    });\n    fireEvent.click(screen.getByRole("button", { name: "Annuler le rangement" }));\n    await waitFor(() => {\n      expect(api.undoOneClickV2).toHaveBeenCalledTimes(1);\n    });\n    expect(api.rollbackExecution).not.toHaveBeenCalled();'''
s = must_replace(s, old, new, "oneclick apply undo assertions")

# The partial-access test still expects a 7-file preview, which is exactly what the
# default v2 plan mock returns for the accessible Desktop root. Replace legacy call assertion.
s = s.replace(
    "    expect(api.registerUserContentRoot).toHaveBeenCalled();\n",
    "    expect(api.buildOneClickV2Plan).toHaveBeenCalled();\n",
)
p.write_text(s)

# --- Milestone12_2 UX ---
p = Path("apps/desktop/src/Milestone12_2Ux.test.tsx")
s = p.read_text()
s = must_replace(
    s,
    "  probeUserContentAccess: vi.fn(),\n  authorizeUserContentFolder: vi.fn(),",
    "  probeUserContentAccess: vi.fn(),\n  buildOneClickV2Plan: vi.fn(),\n  applyOneClickV2: vi.fn(),\n  undoOneClickV2: vi.fn(),\n  authorizeUserContentFolder: vi.fn(),",
    "ux mock functions",
)
anchor = '''    vi.mocked(api.probeUserContentAccess).mockResolvedValue(\n      locations\n        .filter((item) => item.recommended)\n        .map((item) => toProbe(item)),\n    );'''
insert = anchor + '''\n    vi.mocked(api.buildOneClickV2Plan).mockResolvedValue({\n      planId: "v2-plan-zero",\n      filesSeen: 10,\n      proposedMoves: 0,\n      roots: [\n        {\n          kind: "desktop",\n          displayLabel: "Bureau",\n          root: "/Users/local/Desktop",\n          filesSeen: 10,\n          proposedMoves: [],\n          skipped: 10,\n          errors: [],\n        },\n        {\n          kind: "pictures",\n          displayLabel: "Images",\n          root: "/Users/local/Pictures",\n          filesSeen: 0,\n          proposedMoves: [],\n          skipped: 0,\n          errors: ["ACCESS authorization_required: Images — Autorisation nécessaire"],\n        },\n      ],\n    });\n    vi.mocked(api.applyOneClickV2).mockResolvedValue({ applied: [], skipped: 0, errors: [] });\n    vi.mocked(api.undoOneClickV2).mockResolvedValue({ applied: [], skipped: 0, errors: [] });'''
s = must_replace(s, anchor, insert, "ux beforeEach v2 mocks")

# Remove obsolete proposal setup within the partial permission test.
start_marker = '    vi.mocked(api.generateOrganizationProposal).mockResolvedValue({'
start = s.find(start_marker, s.find('it("runs whole computer with partial permission denial and still previews"'))
if start == -1:
    raise SystemExit("ux old proposal setup missing")
end_marker = '    });\n    render(<App />);'
end = s.find(end_marker, start)
if end == -1:
    raise SystemExit("ux old proposal setup end missing")
s = s[:start] + '    render(<App />);' + s[end + len(end_marker):]

old = '''    await waitFor(() => {\n      expect(api.registerUserContentRoot).toHaveBeenCalled();\n      expect(api.scanWorkspace).toHaveBeenCalled();\n      expect(api.generateOrganizationProposal).toHaveBeenCalled();\n    });'''
new = '''    await waitFor(() => {\n      expect(api.buildOneClickV2Plan).toHaveBeenCalledTimes(1);\n    });\n    expect(api.registerUserContentRoot).not.toHaveBeenCalled();\n    expect(api.scanWorkspace).not.toHaveBeenCalled();\n    expect(api.generateOrganizationProposal).not.toHaveBeenCalled();'''
s = must_replace(s, old, new, "ux planning assertions")

# Old execution subsystem must stay unused by v2 preview.
s = s.replace(
    "    expect(api.prepareExecution).not.toHaveBeenCalled();\n",
    "    expect(api.prepareExecution).not.toHaveBeenCalled();\n    expect(api.applyOneClickV2).not.toHaveBeenCalled();\n",
    1,
)
p.write_text(s)

print("one-click v2 UI tests migrated")
