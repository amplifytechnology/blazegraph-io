"""GitHub Release asset downloader for blazegraph-cli binary.

Downloads the correct platform binary on first use in local mode.
Everything is stored inside ``site-packages/blazegraphio/_runtime/``.
"""

from __future__ import annotations

import os
import platform
import shutil
import stat
import sys
from pathlib import Path

import httpx

from blazegraphio.errors import BlazeGraphNotFoundError

_RUNTIME_DIR = Path(__file__).parent / "_runtime"
_BIN_DIR = _RUNTIME_DIR / "bin"
_JRE_DIR = _RUNTIME_DIR / "jre"

_GITHUB_ORG = "amplifytechnology"
_GITHUB_REPO = "blazegraph-io"

# Map (system, machine) to release asset suffix
_PLATFORM_MAP: dict[tuple[str, str], str] = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
}


def _detect_platform() -> str:
    """Return the platform string for the GitHub Release asset name.

    Raises:
        BlazeGraphNotFoundError: If the platform is unsupported.
    """
    system = platform.system()
    machine = platform.machine()
    key = (system, machine)
    if key not in _PLATFORM_MAP:
        raise BlazeGraphNotFoundError(
            f"Unsupported platform: {system}/{machine}. "
            f"Supported: {', '.join(f'{s}/{m}' for s, m in _PLATFORM_MAP)}"
        )
    return _PLATFORM_MAP[key]


def _latest_release_tag() -> str:
    """Fetch the latest release tag from GitHub."""
    url = f"https://api.github.com/repos/{_GITHUB_ORG}/{_GITHUB_REPO}/releases/latest"
    response = httpx.get(url, timeout=30.0, follow_redirects=True)
    response.raise_for_status()
    return response.json()["tag_name"]


def _download_binary(tag: str, platform_str: str) -> Path:
    """Download the blazegraph-cli binary for the given release tag and platform.

    Returns:
        Path to the downloaded binary.
    """
    asset_name = f"blazegraph-cli-{platform_str}"
    url = (
        f"https://github.com/{_GITHUB_ORG}/{_GITHUB_REPO}"
        f"/releases/download/{tag}/{asset_name}"
    )

    _BIN_DIR.mkdir(parents=True, exist_ok=True)
    dest = _BIN_DIR / "blazegraph-cli"

    print(f"Downloading blazegraph-cli {tag} ({platform_str})... ", end="", flush=True)

    with httpx.stream("GET", url, timeout=120.0, follow_redirects=True) as response:
        response.raise_for_status()
        with open(dest, "wb") as f:
            for chunk in response.iter_bytes(chunk_size=8192):
                f.write(chunk)

    # Make executable
    dest.chmod(dest.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    print("done.")
    return dest


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
    local_bin = _BIN_DIR / "blazegraph-cli"
    if local_bin.exists():
        return local_bin

    # 3. Check PATH and cargo bin
    path_bin = shutil.which("blazegraph-cli")
    if path_bin:
        return Path(path_bin)

    cargo_bin = Path.home() / ".cargo" / "bin" / "blazegraph-cli"
    if cargo_bin.exists():
        return cargo_bin

    # 4. Download from GitHub Releases
    try:
        platform_str = _detect_platform()
        tag = _latest_release_tag()
        return _download_binary(tag, platform_str)
    except Exception as exc:
        raise BlazeGraphNotFoundError(
            f"Could not find or download blazegraph-cli: {exc}"
        ) from exc


def get_jre_dir() -> Path:
    """Return the JRE directory path (for ``--jre-path`` flag).

    Creates the directory if it doesn't exist. The CLI itself handles
    downloading the JRE into this directory.
    """
    _JRE_DIR.mkdir(parents=True, exist_ok=True)
    return _JRE_DIR
