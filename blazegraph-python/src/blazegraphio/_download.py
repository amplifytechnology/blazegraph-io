"""GitHub Release asset downloader for blazegraph-cli binary.

Downloads the correct platform archive on first use in local mode,
extracts the CLI binary and Tika JAR into ``site-packages/blazegraphio/_runtime/``.
"""

from __future__ import annotations

import io
import os
import platform
import shutil
import stat
import sys
import tarfile
import zipfile
from pathlib import Path

import httpx

from blazegraphio.errors import BlazeGraphNotFoundError

_RUNTIME_DIR = Path(__file__).parent / "_runtime"
_BIN_DIR = _RUNTIME_DIR / "bin"
_JRE_DIR = _RUNTIME_DIR / "jre"

_GITHUB_ORG = "amplifytechnology"
_GITHUB_REPO = "blazegraph-io"

# Map (system, machine) to (release asset suffix, archive extension)
_PLATFORM_MAP: dict[tuple[str, str], tuple[str, str]] = {
    ("Darwin", "arm64"): ("aarch64-apple-darwin", ".tar.gz"),
    ("Linux", "x86_64"): ("x86_64-unknown-linux-gnu", ".tar.gz"),
    ("Linux", "aarch64"): ("aarch64-unknown-linux-gnu", ".tar.gz"),
    ("Windows", "AMD64"): ("x86_64-pc-windows-msvc", ".zip"),
}


def _detect_platform() -> tuple[str, str]:
    """Return the (platform_suffix, archive_extension) for the GitHub Release asset.

    Raises:
        BlazeGraphNotFoundError: If the platform is unsupported.
    """
    system = platform.system()
    machine = platform.machine()
    key = (system, machine)
    if key not in _PLATFORM_MAP:
        raise BlazeGraphNotFoundError(
            f"Unsupported platform: {system}/{machine}. "
            f"Supported: {', '.join(f'{s}/{m}' for s, m in _PLATFORM_MAP)}. "
            f"macOS Intel users: install via `cargo install` or place blazegraph-cli on PATH."
        )
    return _PLATFORM_MAP[key]


def _latest_release_tag() -> str:
    """Fetch the latest release tag from GitHub."""
    url = f"https://api.github.com/repos/{_GITHUB_ORG}/{_GITHUB_REPO}/releases/latest"
    response = httpx.get(url, timeout=30.0, follow_redirects=True)
    response.raise_for_status()
    return response.json()["tag_name"]


def _download_and_extract(tag: str, platform_str: str, archive_ext: str) -> Path:
    """Download and extract the blazegraph-cli archive for the given release.

    Extracts both the CLI binary and the Tika JAR into _runtime/bin/.

    Returns:
        Path to the extracted CLI binary.
    """
    asset_name = f"blazegraph-cli-{platform_str}{archive_ext}"
    url = (
        f"https://github.com/{_GITHUB_ORG}/{_GITHUB_REPO}"
        f"/releases/download/{tag}/{asset_name}"
    )

    _BIN_DIR.mkdir(parents=True, exist_ok=True)

    is_windows = platform.system() == "Windows"
    cli_name = "blazegraph-cli.exe" if is_windows else "blazegraph-cli"
    dest_binary = _BIN_DIR / cli_name
    dest_jar = _BIN_DIR / "blazing-tika-jni.jar"

    print(f"Downloading blazegraph-cli {tag} ({platform_str})... ", end="", flush=True)

    with httpx.stream("GET", url, timeout=120.0, follow_redirects=True) as response:
        response.raise_for_status()
        archive_bytes = b""
        for chunk in response.iter_bytes(chunk_size=8192):
            archive_bytes += chunk

    # Extract archive
    if archive_ext == ".tar.gz":
        with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:gz") as tar:
            for member in tar.getmembers():
                name = Path(member.name).name
                if name == "blazegraph-cli":
                    with tar.extractfile(member) as f:  # type: ignore[union-attr]
                        dest_binary.write_bytes(f.read())
                elif name == "blazing-tika-jni.jar":
                    with tar.extractfile(member) as f:  # type: ignore[union-attr]
                        dest_jar.write_bytes(f.read())
    elif archive_ext == ".zip":
        with zipfile.ZipFile(io.BytesIO(archive_bytes)) as zf:
            for info in zf.infolist():
                name = Path(info.filename).name
                if name == "blazegraph-cli.exe":
                    dest_binary.write_bytes(zf.read(info.filename))
                elif name == "blazing-tika-jni.jar":
                    dest_jar.write_bytes(zf.read(info.filename))

    # Make binary executable (no-op on Windows)
    if not is_windows:
        dest_binary.chmod(
            dest_binary.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH
        )

    print("done.")
    return dest_binary


def find_or_download_cli() -> Path:
    """Locate the blazegraph-cli binary, downloading if necessary.

    Search order:
    1. ``BLAZEGRAPH_CLI_PATH`` environment variable
    2. ``_runtime/bin/blazegraph-cli`` (package-local, previous download)
    3. PATH / ``~/.cargo/bin/blazegraph-cli``
    4. Download from GitHub Releases

    Returns:
        Path to the binary.

    Raises:
        BlazeGraphNotFoundError: If download fails or platform is unsupported.
    """
    # 1. Environment variable override
    env_path = os.environ.get("BLAZEGRAPH_CLI_PATH")
    if env_path:
        p = Path(env_path)
        if p.exists():
            return p
        raise BlazeGraphNotFoundError(
            f"BLAZEGRAPH_CLI_PATH points to non-existent file: {env_path}"
        )

    # 2. Package-local binary
    is_windows = platform.system() == "Windows"
    cli_name = "blazegraph-cli.exe" if is_windows else "blazegraph-cli"
    local_bin = _BIN_DIR / cli_name
    if local_bin.exists():
        return local_bin

    # 3. Check PATH and cargo bin
    path_bin = shutil.which("blazegraph-cli")
    if path_bin:
        return Path(path_bin)

    cargo_bin = Path.home() / ".cargo" / "bin" / cli_name
    if cargo_bin.exists():
        return cargo_bin

    # 4. Download from GitHub Releases
    try:
        platform_str, archive_ext = _detect_platform()
        tag = _latest_release_tag()
        return _download_and_extract(tag, platform_str, archive_ext)
    except Exception as exc:
        raise BlazeGraphNotFoundError(
            f"Could not find or download blazegraph-cli: {exc}"
        ) from exc


def get_jre_dir() -> Path:
    """Return the JRE directory path (for ``--jre-path`` flag).

    Resolution order:
    1. ``JAVA_HOME`` environment variable (if set and exists)
    2. Package-local ``_runtime/jre/`` directory (for pip-installed JRE)

    Creates the fallback directory if it doesn't exist. The CLI itself
    handles downloading the JRE into the fallback directory.
    """
    java_home = os.environ.get("JAVA_HOME")
    if java_home:
        java_home_path = Path(java_home)
        if java_home_path.is_dir():
            return java_home_path
    _JRE_DIR.mkdir(parents=True, exist_ok=True)
    return _JRE_DIR
