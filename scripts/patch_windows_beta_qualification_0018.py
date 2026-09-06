from pathlib import Path
import re

PERSISTENCE = Path("crates/persistence/src/lib.rs")
M8 = Path("crates/application/tests/milestone8_qualification.rs")


def function_block(text: str, name: str):
    marker = f"    fn {name}() {{"
    start = text.find(marker)
    if start < 0:
        raise SystemExit(f"missing test function: {name}")
    next_test = text.find("\n    #[test]", start + len(marker))
    end = len(text) if next_test < 0 else next_test
    return start, end, text[start:end]


def patch_schema_expectation(text: str, name: str) -> str:
    start, end, block = function_block(text, name)
    original = block
    # Most migration tests store the reopened current version in version/version_after.
    block, count = re.subn(
        r"assert_eq!\((version(?:_after)?), 17\);",
        r"assert_eq!(\1, 18);",
        block,
        count=1,
    )
    if count == 0:
        # monitoring_hardening uses a multiline direct PRAGMA assertion. Only update
        # the final current-version assertion in this test; the legacy v12 fixture
        # assertion must remain 12.
        needle = "\n            17\n        );"
        pos = block.rfind(needle)
        if pos >= 0:
            block = block[:pos] + "\n            18\n        );" + block[pos + len(needle):]
            count = 1
    if count != 1 or block == original:
        raise SystemExit(f"expected exactly one schema-17 assertion in {name}, patched {count}")
    return text[:start] + block + text[end:]


persistence = PERSISTENCE.read_text(encoding="utf-8")
for test_name in [
    "encrypted_migration_is_valid_and_has_no_fk_violations",
    "local_rules_migration_upgrades_version_nine_and_survives_reopen",
    "hybrid_search_migration_upgrades_version_eleven_to_twelve",
    "monitoring_hardening_migrates_version_twelve_without_losing_restore_state",
    "semantic_migration_upgrades_an_existing_version_four_catalog",
]:
    persistence = patch_schema_expectation(persistence, test_name)
PERSISTENCE.write_text(persistence, encoding="utf-8")

m8 = M8.read_text(encoding="utf-8")
needle = """        OperationPrimitiveManifest::CreateDirectory {\n            destination_relative_path,\n        }\n        | OperationPrimitiveManifest::SameVolumeMove {"""
replacement = """        OperationPrimitiveManifest::CreateDirectory {\n            destination_relative_path,\n        } => destination_relative_path,\n        OperationPrimitiveManifest::RemoveDirectoryIfEmpty {\n            source_relative_path,\n        } => source_relative_path,\n        OperationPrimitiveManifest::SameVolumeMove {"""
if m8.count(needle) != 1:
    raise SystemExit(f"expected one primitive_destination match prefix, found {m8.count(needle)}")
m8 = m8.replace(needle, replacement, 1)
M8.write_text(m8, encoding="utf-8")

print("Patched Windows qualification expectations for schema 18 and empty-directory cleanup.")
