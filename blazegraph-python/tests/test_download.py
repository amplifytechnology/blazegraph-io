"""Test platform detection and download logic (mocked HTTP)."""

from __future__ import annotations

import os
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from blazegraphio._download import (
    _PLATFORM_MAP,
    _detect_platform,
    find_or_download_cli,
    get_jre_dir,
)
from blazegraphio.errors import BlazeGraphNotFoundError


class TestDetectPlatform:
    """Test platform detection maps to correct release asset names."""

    @patch("blazegraphio._download.platform.system", return_value="Darwin")
    @patch("blazegraphio._download.platform.machine", return_value="arm64")
    def test_macos_arm64(self, _m, _s) -> None:
        assert _detect_platform() == "aarch64-apple-darwin"

    @patch("blazegraphio._download.platform.system", return_value="Darwin")
    @patch("blazegraphio._download.platform.machine", return_value="x86_64")
    def test_macos_x86(self, _m, _s) -> None:
        assert _detect_platform() == "x86_64-apple-darwin"

    @patch("blazegraphio._download.platform.system", return_value="Linux")
    @patch("blazegraphio._download.platform.machine", return_value="x86_64")
    def test_linux_x86(self, _m, _s) -> None:
        assert _detect_platform() == "x86_64-unknown-linux-gnu"

    @patch("blazegraphio._download.platform.system", return_value="Windows")
    @patch("blazegraphio._download.platform.machine", return_value="AMD64")
    def test_unsupported_platform(self, _m, _s) -> None:
        with pytest.raises(BlazeGraphNotFoundError, match="Unsupported platform"):
            _detect_platform()


class TestFindOrDownloadCli:
    """Test CLI discovery with various environment states."""

    @patch.dict(os.environ, {"BLAZEGRAPH_CLI_PATH": "/usr/local/bin/blazegraph-cli"})
    @patch("blazegraphio._download.Path.exists", return_value=True)
    def test_env_var_found(self, _exists) -> None:
        result = find_or_download_cli()
        assert str(result) == "/usr/local/bin/blazegraph-cli"

    @patch.dict(os.environ, {"BLAZEGRAPH_CLI_PATH": "/nonexistent/blazegraph-cli"})
    def test_env_var_not_found(self) -> None:
        with pytest.raises(BlazeGraphNotFoundError, match="BLAZEGRAPH_CLI_PATH"):
            find_or_download_cli()

    @patch.dict(os.environ, {}, clear=True)
    @patch("blazegraphio._download.shutil.which", return_value=None)
    def test_falls_through_to_download(self, _which, tmp_path: Path) -> None:
        """When nothing is found locally, it attempts download."""
        # Remove BLAZEGRAPH_CLI_PATH from env
        os.environ.pop("BLAZEGRAPH_CLI_PATH", None)

        with patch("blazegraphio._download._BIN_DIR", tmp_path / "bin"):
            with patch(
                "blazegraphio._download._detect_platform",
                side_effect=BlazeGraphNotFoundError("test: unsupported"),
            ):
                with pytest.raises(BlazeGraphNotFoundError, match="Could not find"):
                    find_or_download_cli()

    @patch.dict(os.environ, {}, clear=True)
    @patch("blazegraphio._download.shutil.which", return_value="/usr/bin/blazegraph-cli")
    def test_found_on_path(self, _which, tmp_path: Path) -> None:
        os.environ.pop("BLAZEGRAPH_CLI_PATH", None)
        with patch("blazegraphio._download._BIN_DIR", tmp_path / "bin"):
            result = find_or_download_cli()
            assert str(result) == "/usr/bin/blazegraph-cli"


class TestGetJreDir:
    """Test JRE directory creation."""

    def test_creates_directory(self, tmp_path: Path) -> None:
        jre_dir = tmp_path / "runtime" / "jre"
        with patch("blazegraphio._download._JRE_DIR", jre_dir):
            result = get_jre_dir()
            assert result == jre_dir
            assert jre_dir.exists()
