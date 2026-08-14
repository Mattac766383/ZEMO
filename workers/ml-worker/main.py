"""Optional, network-denied ML sidecar.

The sidecar intentionally ships without a model. A signed model manifest and a
provider implementation must be installed before OCR or vision can run. This
keeps model downloads out of the application runtime and makes provenance
explicit.
"""

from __future__ import annotations

import hashlib
import json
import socket
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NoReturn

PROTOCOL_VERSION = 1
MAX_LINE_BYTES = 32 * 1024 * 1024


def _deny_network(*_args: Any, **_kwargs: Any) -> NoReturn:
    raise RuntimeError("network access is disabled in the ML worker")


socket.create_connection = _deny_network  # type: ignore[assignment]
socket.socket.connect = _deny_network  # type: ignore[assignment]


@dataclass(frozen=True)
class ModelManifest:
    capability: str
    revision: str
    sha256: str
    model_path: Path

    @classmethod
    def load(cls, path: Path) -> "ModelManifest":
        value = json.loads(path.read_text(encoding="utf-8"))
        model_path = path.parent / value["file"]
        digest = hashlib.sha256(model_path.read_bytes()).hexdigest()
        if digest != value["sha256"]:
            raise ValueError("model checksum mismatch")
        return cls(
            capability=value["capability"],
            revision=value["revision"],
            sha256=value["sha256"],
            model_path=model_path,
        )


def respond(payload: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(payload, ensure_ascii=False, separators=(",", ":")))
    sys.stdout.write("\n")
    sys.stdout.flush()


def handle(request: dict[str, Any]) -> dict[str, Any]:
    request_id = request.get("requestId")
    if request.get("protocolVersion") != PROTOCOL_VERSION:
        return {
            "protocolVersion": PROTOCOL_VERSION,
            "requestId": request_id,
            "status": "error",
            "code": "unsupported_protocol",
        }
    if request.get("method") == "health":
        return {
            "protocolVersion": PROTOCOL_VERSION,
            "requestId": request_id,
            "status": "ok",
            "network": False,
            "models": [],
        }
    return {
        "protocolVersion": PROTOCOL_VERSION,
        "requestId": request_id,
        "status": "error",
        "code": "signed_local_model_required",
    }


def main() -> int:
    for raw_line in sys.stdin.buffer:
        if len(raw_line) > MAX_LINE_BYTES:
            respond(
                {
                    "protocolVersion": PROTOCOL_VERSION,
                    "status": "error",
                    "code": "request_too_large",
                }
            )
            continue
        try:
            request = json.loads(raw_line)
            if not isinstance(request, dict):
                raise TypeError("request must be an object")
            respond(handle(request))
        except (json.JSONDecodeError, TypeError, ValueError):
            respond(
                {
                    "protocolVersion": PROTOCOL_VERSION,
                    "status": "error",
                    "code": "invalid_request",
                }
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
