from pathlib import Path
import json

worker = Path('workers/operation-executor/src/lib.rs')
text = worker.read_text()
old = '''        OperationPrimitiveManifest::CreateDirectory { .. }\n        | OperationPrimitiveManifest::InternalStage { .. } => return false,'''
new = '''        OperationPrimitiveManifest::CreateDirectory { .. }\n        | OperationPrimitiveManifest::RemoveDirectoryIfEmpty { .. }\n        | OperationPrimitiveManifest::InternalStage { .. } => return false,'''
count = text.count(old)
if count != 2:
    raise SystemExit(f'expected 2 case-only guard matches, found {count}')
worker.write_text(text.replace(old, new))

package = Path('package.json')
data = json.loads(package.read_text())
overrides = data.setdefault('overrides', {})
overrides['browserslist'] = '4.28.7'
package.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')

print('patched operation-executor case-only guards and pinned browserslist 4.28.7')
