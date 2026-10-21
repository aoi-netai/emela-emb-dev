#!/usr/bin/env python3
"""Fetch pinned ESP32-C3 model/toolchain dependencies into an external cache."""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

MODEL_URL = "https://github.com/mithro/renode-espemu.git"
MODEL_COMMIT = "9c9bdf6bb9c80d40efeaa8e4e398668e60c7007e"
TOOLCHAIN_VERSION = "esp-14.2.0_20241119"
TOOLCHAIN_URL = "https://github.com/espressif/crosstool-NG/releases/download/esp-14.2.0_20241119/riscv32-esp-elf-14.2.0_20241119-x86_64-linux-gnu.tar.xz"
TOOLCHAIN_SHA256 = "7faaa86d272f3e43c233f8a5ffeba327673224a752c2eb72394655d5e7950000"
ROM_URL = "https://github.com/espressif/esp-rom-elfs/releases/download/20240305/esp-rom-elfs-20240305.tar.gz"
ROM_ARCHIVE_SHA256 = "a26609b415710f0163d785850c769752717004059c129c472e9a0cbd54e0422c"
ROM_SHA256 = "19ac22e08707df926fb0cf4c54795d4067b983fae8635f396ded173a6d78fc3c"


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def default_cache():
    return Path(os.environ.get("EMELA_ESP32C3_CACHE", "/tmp/emela-esp32c3-cache"))


@contextlib.contextmanager
def cache_lock(cache):
    cache.mkdir(parents=True, exist_ok=True)
    with (cache / ".setup.lock").open("a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


def git(*args):
    result = subprocess.run(["git", *map(str, args)], check=True, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=600)
    return result.stdout.strip()


def verify_models(path):
    path = Path(path).resolve()
    if git("-C", path, "rev-parse", "HEAD") != MODEL_COMMIT:
        raise ValueError(f"model checkout must be at {MODEL_COMMIT}: {path}")
    if git("-C", path, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError(f"model checkout has local changes: {path}")
    required = ["gpio/ESP32C3_GPIO.cs", "uart/ESP32C3_UART.cs",
                "systimer/ESP32C3_SysTimer.cs", "interrupt-matrix/ESP32C3_InterruptMatrix.cs"]
    if any(not (path / "peripherals" / name).is_file() for name in required):
        raise ValueError(f"model checkout is incomplete: {path}")
    return path


def ensure_models(cache, source=None):
    if source is not None:
        return verify_models(source)
    destination = cache / ("renode-espemu-" + MODEL_COMMIT)
    with cache_lock(cache):
        if not destination.exists():
            with tempfile.TemporaryDirectory(prefix="models-", dir=cache) as temporary:
                checkout = Path(temporary) / "source"
                git("init", "--quiet", checkout)
                git("-C", checkout, "remote", "add", "origin", MODEL_URL)
                git("-C", checkout, "fetch", "--quiet", "--depth=1", "origin", MODEL_COMMIT)
                git("-C", checkout, "checkout", "--quiet", "--detach", "FETCH_HEAD")
                verify_models(checkout)
                checkout.rename(destination)
        return verify_models(destination)


def download(cache, name, url, expected_hash):
    archive = cache / "downloads" / name
    archive.parent.mkdir(parents=True, exist_ok=True)
    if archive.is_file() and sha256(archive) == expected_hash:
        return archive
    with tempfile.NamedTemporaryFile(prefix="download-", dir=archive.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            with urllib.request.urlopen(url, timeout=120) as response:
                shutil.copyfileobj(response, stream)
            stream.close()
            if sha256(temporary) != expected_hash:
                raise ValueError(f"SHA256 mismatch for {url}")
            temporary.replace(archive)
        finally:
            temporary.unlink(missing_ok=True)
    return archive


def extract(archive, destination):
    # The digest is checked first; also reject archive paths escaping the cache.
    root = destination.resolve()
    with tarfile.open(archive) as source:
        for member in source.getmembers():
            target = (root / member.name).resolve()
            if not target.is_relative_to(root) or member.isdev():
                raise ValueError(f"unsafe archive entry: {member.name}")
            if member.issym() or member.islnk():
                base = target.parent if member.issym() else root
                if not (base / member.linkname).resolve().is_relative_to(root):
                    raise ValueError(f"unsafe archive link: {member.name}")
        source.extractall(root)


def ensure_toolchain(cache, source=None):
    if source is not None:
        path = Path(source).resolve()
        if not (path / "bin/riscv32-esp-elf-gcc").is_file():
            raise FileNotFoundError(f"missing riscv32-esp-elf-gcc under {path}/bin")
        return path
    if os.uname().sysname != "Linux" or os.uname().machine not in {"x86_64", "amd64"}:
        raise ValueError("automatic toolchain setup supports Linux x86_64; use --toolchain on this host")
    destination = cache / ("toolchain-" + TOOLCHAIN_VERSION)
    with cache_lock(cache):
        stamp = destination / ".archive-sha256"
        if not stamp.is_file() or stamp.read_text().strip() != TOOLCHAIN_SHA256:
            if destination.exists():
                raise ValueError(f"unverified toolchain cache exists: {destination}")
            archive = download(cache, TOOLCHAIN_URL.rsplit("/", 1)[1], TOOLCHAIN_URL, TOOLCHAIN_SHA256)
            with tempfile.TemporaryDirectory(prefix="toolchain-", dir=cache) as temporary:
                extract(archive, Path(temporary))
                extracted = Path(temporary) / "riscv32-esp-elf"
                if not (extracted / "bin/riscv32-esp-elf-gcc").is_file():
                    raise ValueError("toolchain archive did not contain the expected compiler")
                (extracted / ".archive-sha256").write_text(TOOLCHAIN_SHA256 + "\n")
                extracted.rename(destination)
        return ensure_toolchain(cache, destination)


def ensure_rom(cache, source=None):
    if source is not None:
        path = Path(source).resolve()
        if not path.is_file() or sha256(path) != ROM_SHA256:
            raise ValueError(f"ESP32-C3 revision 3 ROM ELF SHA256 mismatch: {path}")
        return path
    destination = cache / "esp32c3_rev3_rom.elf"
    with cache_lock(cache):
        if not destination.is_file() or sha256(destination) != ROM_SHA256:
            archive = download(cache, ROM_URL.rsplit("/", 1)[1], ROM_URL, ROM_ARCHIVE_SHA256)
            with tempfile.TemporaryDirectory(prefix="rom-", dir=cache) as temporary:
                extract(archive, Path(temporary))
                candidates = list(Path(temporary).rglob("esp32c3_rev3_rom.elf"))
                if len(candidates) != 1 or sha256(candidates[0]) != ROM_SHA256:
                    raise ValueError("ROM archive did not contain the expected ESP32-C3 revision 3 ELF")
                shutil.copyfile(candidates[0], destination)
        return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=default_cache())
    parser.add_argument("--models", type=Path, help="reuse a clean checkout at the pinned commit")
    parser.add_argument("--toolchain", type=Path, default=os.environ.get("RISCV32_ESP_TOOLCHAIN"))
    parser.add_argument("--with-rom", action="store_true", help="also fetch the optional SDK Hello ROM")
    parser.add_argument("--rom", type=Path)
    args = parser.parse_args()
    result = {"model_commit": MODEL_COMMIT, "models": str(ensure_models(args.cache, args.models)),
              "toolchain": str(ensure_toolchain(args.cache, args.toolchain))}
    if args.with_rom:
        result["rom"] = str(ensure_rom(args.cache, args.rom))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
