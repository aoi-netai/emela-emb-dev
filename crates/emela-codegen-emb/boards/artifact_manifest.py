"""Read the versioned generated-C contract without importing compiler or IR code."""

import json
from pathlib import Path

MANIFEST_FILE = "emela_artifact.json"
INTERFACES = {"public_header": "emela_embedded.h", "platform_header": "emela_board.h",
              "aliases_header": "emela_embedded_exports.h"}


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate manifest key: {key}")
        result[key] = value
    return result


def load_manifest(directory, require_aliases=False):
    """Return validated role -> ordered paths; never discover extra sources by glob.

    C ABI v1 fixes the three interface header names. Aliases are optional for a C
    library and required by a BSP. All declared paths must be existing files within
    the artifact directory, including when symlinks are present.
    """
    directory = Path(directory).resolve()
    manifest = directory / MANIFEST_FILE
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"), object_pairs_hook=_object)
        if not isinstance(data, dict) or set(data) != {"schema_version", "abi_version", "files"}:
            raise ValueError("expected schema_version, abi_version, and files")
        if any(type(data[key]) is not int or data[key] != 1 for key in ("schema_version", "abi_version")):
            raise ValueError("unsupported schema/ABI version (expected 1/1)")
        if not isinstance(data["files"], list):
            raise ValueError("files must be a list")
        roles = {role: [] for role in ["source", "header", *INTERFACES]}
        seen = set()
        for entry in data["files"]:
            if not isinstance(entry, dict) or set(entry) != {"path", "role"}:
                raise ValueError("each file needs path and role")
            name, role = entry["path"], entry["role"]
            if (not isinstance(name, str) or any(char in name for char in "\\:\0")
                    or any(part in {"", ".", ".."} for part in name.split("/"))):
                raise ValueError(f"invalid relative path: {name!r}")
            if name in seen:
                raise ValueError(f"duplicate path: {name}")
            seen.add(name)
            if not isinstance(role, str) or role not in roles:
                raise ValueError(f"unknown file role: {role!r}")
            if role in INTERFACES:
                if name != INTERFACES[role] or roles[role]:
                    raise ValueError(f"invalid interface role: {name}")
            elif role == "source":
                if not name.endswith(".c"):
                    raise ValueError("source must have the .c suffix")
            elif not name.endswith(".h") or name in INTERFACES.values():
                raise ValueError(f"invalid header role: {name}")
            path = directory / name
            if directory not in path.resolve().parents or not path.is_file():
                raise ValueError(f"missing or escaping generated file: {name}")
            roles[role].append(path)
        required = ["source", "public_header", "platform_header"]
        if require_aliases:
            required.append("aliases_header")
        if any(not roles[role] for role in required):
            raise ValueError("required file roles: " + ", ".join(required))
        return roles
    except (OSError, ValueError, TypeError) as error:
        raise ValueError(f"invalid generated artifact {manifest}: {error}") from error


def include_directories(inputs):
    """Include the artifact root separately; also support nested generated files."""
    return {path.parent for paths in inputs.values() for path in paths}
