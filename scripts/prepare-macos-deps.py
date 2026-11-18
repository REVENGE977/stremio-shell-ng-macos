#!/usr/bin/env python3
"""
Cross-arch macOS dependency stager, bundler, and Mach-O verifier.

Allows an Intel (x86_64) MacPorts macOS host/VM to cross-compile and package
a completely self-contained Apple Silicon (arm64) or Universal 2 .app bundle by
fetching prebuilt arm64 MacPorts .tbz2 archives from packages.macports.org,
recursively resolving the full dynamic library tree, rewriting Mach-O install
names, ad-hoc signing every modified binary/dylib, and verifying zero external
/opt/local or missing @rpath dependencies remain.
"""

from __future__ import annotations

import argparse
import glob
import html.parser
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.error
import urllib.request
from collections import deque
from pathlib import Path
from typing import Iterable

MACPORTS_PACKAGES_URL = "https://packages.macports.org"

SYSTEM_LIB_PREFIXES = (
    "/System/Library/",
    "/usr/lib/",
)

# Fallback MacPorts port names for shared libraries whose soversion or path may
# differ between the host VM's MacPorts tree and the downloaded arm64 archive.
KNOWN_LIB_TO_PORT: dict[str, list[str]] = {
    "libmpv": ["mpv"],
    "libavcodec": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libavdevice": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libavfilter": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libavformat": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libavutil": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libpostproc": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libswresample": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libswscale": ["ffmpeg7", "ffmpeg6", "ffmpeg8", "ffmpeg"],
    "libass": ["libass"],
    "libplacebo": ["libplacebo"],
    "libvulkan": ["vulkan-loader"],
    "libMoltenVK": ["molten-vk"],
    "libshaderc_shared": ["shaderc"],
    "libSPIRV-Tools": ["spirv-tools"],
    "libSPIRV-Tools-shared": ["spirv-tools"],
    "libdav1d": ["dav1d"],
    "libaom": ["aom"],
    "libSvtAv1Enc": ["svt-av1"],
    "librav1e": ["rav1e"],
    "libvpx": ["libvpx"],
    "libx264": ["x264"],
    "libx265": ["x265"],
    "libopus": ["opus"],
    "libvorbis": ["libvorbis"],
    "libvorbisenc": ["libvorbis"],
    "libogg": ["libogg"],
    "libflac": ["flac"],
    "libFLAC": ["flac"],
    "libmp3lame": ["lame"],
    "libsoxr": ["soxr"],
    "libspeex": ["speex"],
    "libtheora": ["libtheora"],
    "libtheoraenc": ["libtheora"],
    "libtheoradec": ["libtheora"],
    "libwebp": ["webp"],
    "libwebpmux": ["webp"],
    "libsharpyuv": ["webp"],
    "libz": ["zlib"],
    "libbz2": ["bzip2"],
    "liblzma": ["xz"],
    "libzstd": ["zstd"],
    "libiconv": ["libiconv"],
    "libcharset": ["libiconv"],
    "libintl": ["gettext-runtime"],
    "libfreetype": ["freetype"],
    "libharfbuzz": ["harfbuzz"],
    "libfribidi": ["fribidi"],
    "libfontconfig": ["fontconfig"],
    "libexpat": ["expat"],
    "libunibreak": ["libunibreak"],
    "liblcms2": ["lcms2"],
    "libarchive": ["libarchive"],
    "libuchardet": ["uchardet"],
    "librubberband": ["rubberband"],
    "libsamplerate": ["libsamplerate"],
    "libfftw3": ["fftw-3"],
    "libbluray": ["libbluray"],
    "libdvdnav": ["libdvdnav"],
    "libdvdread": ["libdvdread"],
    "libdvdcss": ["libdvdcss"],
    "libcdio": ["libcdio"],
    "libcdio_paranoia": ["libcdio-paranoia"],
    "libcdio_cdda": ["libcdio-paranoia"],
    "libluajit-5.1": ["luajit"],
    "liblua": ["lua"],
    "libssl": ["openssl3", "openssl"],
    "libcrypto": ["openssl3", "openssl"],
    "libgnutls": ["gnutls"],
    "libp11-kit": ["p11-kit"],
    "libtasn1": ["libtasn1"],
    "libnettle": ["nettle"],
    "libhogweed": ["nettle"],
    "libgmp": ["gmp"],
    "libidn2": ["libidn2"],
    "libunistring": ["libunistring"],
    "libffi": ["libffi"],
    "libbrotlidec": ["brotli"],
    "libbrotlienc": ["brotli"],
    "libbrotlicommon": ["brotli"],
    "libpng16": ["libpng"],
    "libjpeg": ["libjpeg-turbo"],
    "libturbojpeg": ["libjpeg-turbo"],
    "libtiff": ["tiff"],
    "libglib-2.0": ["glib2"],
    "libgobject-2.0": ["glib2"],
    "libgio-2.0": ["glib2"],
    "libgmodule-2.0": ["glib2"],
    "libpcre2-8": ["pcre2"],
    "libxml2": ["libxml2"],
    "liblz4": ["lz4"],
    "libb2": ["libb2"],
    "libnghttp2": ["nghttp2"],
    "libcares": ["c-ares"],
    "libuv": ["libuv"],
    "libicudata": ["icu"],
    "libicui18n": ["icu"],
    "libicuuc": ["icu"],
    "libsrt": ["srt"],
    "libzmq": ["zmq"],
    "libopenjp2": ["openjpeg"],
    "libjxl": ["libjxl"],
    "libjxl_threads": ["libjxl"],
    "libjxl_cms": ["libjxl"],
    "libhwy": ["highway"],
    "libsnappy": ["snappy"],
    "libzvbi": ["zvbi"],
    "libvidstab": ["libvidstab"],
    "libxvidcore": ["XviD"],
    "libmodplug": ["libmodplug"],
    "libopenmpt": ["libopenmpt"],
    "libmpg123": ["mpg123"],
    "librtmp": ["rtmpdump"],
    "libssh": ["libssh"],
    "libsmbclient": ["samba3"],
    "libSDL2-2.0": ["libsdl2"],
    "libc++": ["libcxx"],
    "libc++abi": ["libcxx"],
}


class LinkParser(html.parser.HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.hrefs: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag == "a":
            for key, val in attrs:
                if key == "href" and val:
                    self.hrefs.append(val)


def run_cmd(cmd: list[str], check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, check=check, text=True, capture_output=True)


def lipo_archs(path: Path) -> list[str]:
    if not path.exists():
        return []
    res = subprocess.run(["lipo", "-archs", str(path)], text=True, capture_output=True)
    if res.returncode != 0:
        return []
    return res.stdout.strip().split()


def is_macho(path: Path) -> bool:
    if not path.exists() or path.is_dir():
        return False
    res = subprocess.run(["file", "-b", str(path)], text=True, capture_output=True)
    return "Mach-O" in res.stdout


def parse_otool(path: Path) -> tuple[str | None, list[str], list[str]]:
    """Return (install_id, deps, rpaths) for a single-arch or universal Mach-O binary."""
    res_l = run_cmd(["otool", "-L", str(path)], check=False)
    lines = res_l.stdout.splitlines() if res_l.returncode == 0 else []

    install_ids: set[str] = set()
    res_id = run_cmd(["otool", "-D", str(path)], check=False)
    if res_id.returncode == 0:
        for raw in res_id.stdout.splitlines():
            stripped = raw.strip()
            if stripped and not stripped.endswith(":"):
                install_ids.add(stripped)

    install_id: str | None = next(iter(install_ids), None)
    deps: list[str] = []
    seen_deps: set[str] = set()
    for raw in lines:
        # otool -L header lines are non-indented and end with ':' (including '(architecture ...):')
        if not raw.startswith(("\t", " ")) or raw.rstrip().endswith(":"):
            continue
        stripped = raw.strip()
        if not stripped:
            continue
        lib_path = re.sub(r"\s+\(compatibility version .*$", "", stripped)
        if lib_path in install_ids:
            continue
        if lib_path not in seen_deps:
            seen_deps.add(lib_path)
            deps.append(lib_path)

    rpaths: list[str] = []
    res_load = run_cmd(["otool", "-l", str(path)], check=False)
    if res_load.returncode == 0:
        load_lines = res_load.stdout.splitlines()
        for i, line in enumerate(load_lines):
            if "cmd LC_RPATH" in line:
                for j in range(i + 1, min(i + 6, len(load_lines))):
                    m = re.match(r"^\s*path\s+(.*?)\s+\(offset", load_lines[j])
                    if m and m.group(1) not in rpaths:
                        rpaths.append(m.group(1))
                        break

    return install_id, deps, rpaths


def lipo_merge_pair(x64_path: Path, arm64_path: Path, dest: Path, tmp_dir: Path) -> None:
    """Safely create a Universal (x86_64 + arm64) Mach-O even if either input is already fat."""
    x64_archs = lipo_archs(x64_path)
    arm64_archs = lipo_archs(arm64_path)
    if "x86_64" in x64_archs and "arm64" in x64_archs:
        shutil.copy2(x64_path, dest)
        return
    if "x86_64" in arm64_archs and "arm64" in arm64_archs:
        shutil.copy2(arm64_path, dest)
        return

    thin_x64 = x64_path
    thin_arm64 = arm64_path
    if len(x64_archs) > 1:
        thin_x64 = tmp_dir / f"{dest.name}.x86_64"
        run_cmd(["lipo", str(x64_path), "-thin", "x86_64", "-output", str(thin_x64)])
    if len(arm64_archs) > 1:
        thin_arm64 = tmp_dir / f"{dest.name}.arm64"
        run_cmd(["lipo", str(arm64_path), "-thin", "arm64", "-output", str(thin_arm64)])
    run_cmd(["lipo", "-create", str(thin_x64), str(thin_arm64), "-output", str(dest)])


def is_system_lib(dep: str) -> bool:
    return dep.startswith(SYSTEM_LIB_PREFIXES)


def lib_stem(lib_name: str) -> str:
    """Turn 'libass.9.dylib' or 'libavcodec.61.19.101.dylib' into 'libass' / 'libavcodec'."""
    name = Path(lib_name).name
    if name.endswith(".dylib"):
        name = name[:-6]
    # Strip trailing dotted version components (preserve e.g. libglib-2.0 -> libglib-2.0)
    parts = name.split(".")
    while len(parts) > 1 and parts[-1].isdigit():
        parts.pop()
    return ".".join(parts)


def version_sort_key(ver: str) -> list[tuple[int, int | str]]:
    tokens = re.findall(r"\d+|[A-Za-z]+", ver)
    key: list[tuple[int, int | str]] = []
    for tok in tokens:
        if tok.isdigit():
            key.append((0, int(tok)))
        else:
            key.append((1, tok))
    return key


def parse_archive_filename(port: str, filename: str) -> dict | None:
    if not filename.endswith(".tbz2") or filename.endswith(".tbz2.rmd160"):
        return None
    prefix = f"{port}-"
    if not filename.startswith(prefix):
        return None
    rest = filename[len(prefix) : -len(".tbz2")]
    # Expected format: <version>[+<variants>].darwin_<ver>.<arch> or .darwin_any.noarch
    m = re.match(r"^(?P<ver_var>.+?)\.(?P<os>darwin_(?:\d+|any))\.(?P<arch>[^.]+)$", rest)
    if not m:
        return None
    ver_var = m.group("ver_var")
    os_tag = m.group("os")
    arch_tag = m.group("arch")
    if arch_tag not in ("arm64", "arm64+x86_64", "x86_64+arm64", "universal", "noarch"):
        return None
    if "+" in ver_var:
        ver, variants = ver_var.split("+", 1)
    else:
        ver, variants = ver_var, ""
    return {
        "filename": filename,
        "version": ver,
        "variants": variants,
        "os_tag": os_tag,
        "arch_tag": arch_tag,
    }


def download_url_to_file(url: str, dest: Path) -> None:
    curl_bin = shutil.which("curl")
    if curl_bin:
        res = subprocess.run(
            [
                curl_bin,
                "-sSL",
                "--retry",
                "3",
                "--retry-delay",
                "2",
                "--connect-timeout",
                "30",
                "--max-time",
                "300",
                "-A",
                "stremio-enhanced-ng-macos-builder/1.0",
                "-o",
                str(dest),
                "-w",
                "%{http_code}",
                url,
            ],
            text=True,
            capture_output=True,
        )
        if res.returncode != 0:
            dest.unlink(missing_ok=True)
            raise RuntimeError(f"curl failed ({res.returncode}) for {url}: {res.stderr.strip()}")
        status_code = int((res.stdout or "0").strip() or "0")
        if status_code < 200 or status_code >= 400:
            dest.unlink(missing_ok=True)
            raise urllib.error.HTTPError(url, status_code, f"HTTP Error {status_code}", None, None)
        return

    req = urllib.request.Request(
        url,
        headers={"User-Agent": "stremio-enhanced-ng-macos-builder/1.0"},
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        dest.write_bytes(resp.read())


def fetch_url_bytes(url: str) -> bytes:
    curl_bin = shutil.which("curl")
    if curl_bin:
        import tempfile

        with tempfile.NamedTemporaryFile(delete=False) as tf:
            tmp_path = Path(tf.name)
        try:
            download_url_to_file(url, tmp_path)
            return tmp_path.read_bytes()
        finally:
            tmp_path.unlink(missing_ok=True)

    req = urllib.request.Request(
        url,
        headers={"User-Agent": "stremio-enhanced-ng-macos-builder/1.0"},
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read()


class MacPortsArm64Stager:
    def __init__(
        self,
        macports_prefix: Path,
        stage_dir: Path,
        cache_dir: Path,
        target_darwins: list[str],
    ) -> None:
        self.macports_prefix = macports_prefix
        self.stage_dir = stage_dir
        self.cache_dir = cache_dir
        self.target_darwins = target_darwins
        self.pinned_darwin: str | None = None
        self.fetched_ports: set[str] = set()
        self.host_port_cache: dict[str, str | None] = {}
        self.host_variant_cache: dict[str, set[str]] = {}
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self.stage_dir.mkdir(parents=True, exist_ok=True)

    def stage_path(self, abs_macports_path: str) -> Path:
        rel = abs_macports_path.lstrip("/")
        return self.stage_dir / rel

    def host_installed_variants(self, port: str) -> set[str]:
        if port in self.host_variant_cache:
            return self.host_variant_cache[port]
        res = subprocess.run(["port", "installed", port], text=True, capture_output=True)
        variants: set[str] = set()
        if res.returncode == 0:
            for line in res.stdout.splitlines():
                if "(active)" in line and "+" in line:
                    m = re.search(r"@[^+\s]+\+([^\s]+)", line)
                    if m:
                        variants.update(m.group(1).split("+"))
                    break
        self.host_variant_cache[port] = variants
        return variants

    def select_best_archive(self, port: str) -> tuple[str, str] | None:
        url = f"{MACPORTS_PACKAGES_URL}/{port}/"
        try:
            html_bytes = fetch_url_bytes(url)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            raise
        parser = LinkParser()
        parser.feed(html_bytes.decode("utf-8", errors="replace"))

        entries = []
        for href in parser.hrefs:
            name = href.rsplit("/", 1)[-1]
            parsed = parse_archive_filename(port, name)
            if parsed:
                entries.append(parsed)
        if not entries:
            return None

        darwin_order: list[str] = []
        if self.pinned_darwin:
            darwin_order.append(self.pinned_darwin)
        for d in self.target_darwins:
            if d not in darwin_order:
                darwin_order.append(d)
        darwin_order.append("darwin_any")

        host_vars = self.host_installed_variants(port)

        def score(item: dict) -> tuple:
            os_tag = item["os_tag"]
            if os_tag in darwin_order:
                os_rank = -darwin_order.index(os_tag)
            else:
                # Unknown darwin fallback: prefer higher darwin number closest to target
                m = re.match(r"darwin_(\d+)", os_tag)
                os_rank = -100 + (int(m.group(1)) if m else -1000)

            arch_pref = {
                "arm64": 3,
                "arm64+x86_64": 2,
                "x86_64+arm64": 2,
                "universal": 2,
                "noarch": 1,
            }.get(item["arch_tag"], 0)

            item_vars = set(item["variants"].split("+")) if item["variants"] else set()
            # Prefer matching host variants when available, otherwise minimal variants,
            # and avoid x11/debug variants when non-x11 defaults exist.
            x11_penalty = -1 if ("x11" in item_vars and "x11" not in host_vars) else 0
            host_overlap = len(item_vars & host_vars) if host_vars else 0
            return (
                os_rank,
                arch_pref,
                x11_penalty,
                version_sort_key(item["version"]),
                host_overlap,
                -len(item["variants"]),
            )

        best = max(entries, key=score)
        if not self.pinned_darwin and best["os_tag"] != "darwin_any":
            self.pinned_darwin = best["os_tag"]
            print(f"[arm64-stage] Pinned MacPorts archive epoch to {self.pinned_darwin}")
        return best["filename"], f"{url}{best['filename']}"

    def download_and_extract_port(self, port: str) -> bool:
        if port in self.fetched_ports:
            return True
        selected = self.select_best_archive(port)
        if not selected:
            print(f"[arm64-stage] WARN: No prebuilt arm64 MacPorts archive found for '{port}'")
            self.fetched_ports.add(port)
            return False

        filename, url = selected
        cached_tbz2 = self.cache_dir / filename
        if not cached_tbz2.exists():
            print(f"[arm64-stage] Downloading {filename} ...")
            tmp_path = cached_tbz2.with_suffix(".tbz2.part")
            download_url_to_file(url, tmp_path)
            tmp_path.replace(cached_tbz2)
        else:
            print(f"[arm64-stage] Using cached {filename}")

        with tarfile.open(cached_tbz2, "r:bz2") as tf:
            for member in tf.getmembers():
                clean_name = member.name.lstrip("./")
                if not clean_name or clean_name.startswith("/") or ".." in Path(clean_name).parts:
                    continue
                target = self.stage_dir / clean_name
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                elif member.issym():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    if target.exists() or target.is_symlink():
                        target.unlink()
                    link_target = member.linkname
                    if link_target.startswith("/"):
                        # Keep symlinks rooted inside the staging tree if MacPorts used an absolute link.
                        rel_to_parent = os.path.relpath(self.stage_dir / link_target.lstrip("/"), target.parent)
                        link_target = rel_to_parent
                    target.symlink_to(link_target)
                elif member.isreg():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    if target.exists() or target.is_symlink():
                        target.unlink()
                    src = tf.extractfile(member)
                    if src is not None:
                        with src, open(target, "wb") as out_f:
                            shutil.copyfileobj(src, out_f)
                        os.chmod(target, member.mode & 0o777)

        self.fetched_ports.add(port)
        return True

    def port_provides(self, file_path: str) -> str | None:
        if file_path in self.host_port_cache:
            return self.host_port_cache[file_path]
        res = subprocess.run(["port", "provides", file_path], text=True, capture_output=True)
        port_name: str | None = None
        if res.returncode == 0:
            for line in res.stdout.splitlines():
                m = re.search(r"is provided by:\s*(\S+)", line)
                if m:
                    port_name = m.group(1)
                    break
        self.host_port_cache[file_path] = port_name
        return port_name

    def resolve_ports_for_dep(self, dep_abs_path: str) -> list[str]:
        candidates: list[str] = []

        def add(p: str | None) -> None:
            if p and p not in candidates:
                candidates.append(p)

        # 1. Exact path on host MacPorts
        if Path(dep_abs_path).exists():
            add(self.port_provides(dep_abs_path))

        # 2. Libexec directory port name (e.g. /opt/local/libexec/ffmpeg7/lib/...)
        m_libexec = re.search(r"/libexec/([^/]+)/lib/", dep_abs_path)
        if m_libexec:
            add(m_libexec.group(1))

        # 3. Stem match against host MacPorts libs
        stem = lib_stem(dep_abs_path)
        for pattern in (
            f"{self.macports_prefix}/lib/{stem}*.dylib",
            f"{self.macports_prefix}/libexec/*/lib/{stem}*.dylib",
        ):
            for match in glob.glob(pattern):
                add(self.port_provides(match))

        # 4. Explicit common MacPorts stem map
        for mapped in KNOWN_LIB_TO_PORT.get(stem, []):
            add(mapped)

        # 5. Direct stem guesses (e.g. libbluray -> libbluray, zlib -> zlib, dav1d -> dav1d)
        add(stem)
        if stem.startswith("lib"):
            add(stem[3:])

        return candidates

    def resolve_staged_dep(self, dep: str, current_file: Path) -> Path | None:
        if is_system_lib(dep):
            return None

        candidate_abs: str | None = None
        if dep.startswith("@loader_path/"):
            rel = dep[len("@loader_path/") :]
            staged_rel = current_file.parent / rel
            if staged_rel.exists():
                return staged_rel.resolve()
            # Map back to virtual /opt/local/... path
            candidate_abs = "/" + str(staged_rel.resolve().relative_to(self.stage_dir.resolve()))
        elif dep.startswith("@rpath/"):
            rel = dep[len("@rpath/") :]
            for pattern in (
                str(self.stage_dir / self.macports_prefix.as_posix().lstrip("/") / "lib" / rel),
                str(self.stage_dir / self.macports_prefix.as_posix().lstrip("/") / "libexec" / "*" / "lib" / rel),
            ):
                hits = glob.glob(pattern)
                if hits:
                    return Path(hits[0]).resolve()
            candidate_abs = f"{self.macports_prefix}/lib/{rel}"
        elif dep.startswith("/"):
            candidate_abs = dep

        if not candidate_abs:
            return None

        staged = self.stage_path(candidate_abs)
        if staged.exists():
            return staged.resolve()

        # Fetch missing port providing this dependency and re-check
        for port_candidate in self.resolve_ports_for_dep(candidate_abs):
            if self.download_and_extract_port(port_candidate):
                if staged.exists():
                    return staged.resolve()
                # Check if the file lives inside libexec/*/lib/ or /opt/local/lib/
                name = Path(candidate_abs).name
                for fallback in glob.glob(
                    str(self.stage_dir / self.macports_prefix.as_posix().lstrip("/") / "**" / name),
                    recursive=True,
                ):
                    p = Path(fallback)
                    if p.exists():
                        return p.resolve()

        return None

    def stage_seed_binaries_and_walk(self, seed_ports: Iterable[str]) -> None:
        for port in seed_ports:
            self.download_and_extract_port(port)

        queue: deque[Path] = deque()
        visited: set[Path] = set()

        lib_dir = self.stage_path(f"{self.macports_prefix}/lib")
        for candidate in (
            lib_dir / "libmpv.2.dylib",
            lib_dir / "libmpv.dylib",
        ):
            if candidate.exists():
                queue.append(candidate.resolve())

        # Also queue staged arm64 CLI tools (node, ffmpeg, ffprobe) if present
        bin_dir = self.stage_path(f"{self.macports_prefix}/bin")
        for tool_name in ("node", "ffmpeg", "ffprobe"):
            p = bin_dir / tool_name
            if p.exists():
                queue.append(p.resolve())
        for ffmpeg_bin in glob.glob(
            str(self.stage_dir / self.macports_prefix.as_posix().lstrip("/") / "libexec" / "ffmpeg*" / "bin" / "ff*")
        ):
            p = Path(ffmpeg_bin)
            if p.exists():
                queue.append(p.resolve())

        missing: list[tuple[Path, str]] = []
        while queue:
            cur = queue.popleft()
            if cur in visited or not cur.exists():
                continue
            visited.add(cur)

            if not is_macho(cur):
                continue

            archs = lipo_archs(cur)
            if "arm64" not in archs:
                raise RuntimeError(f"Staged binary/library {cur} lacks arm64 slice (found: {archs})")

            _, deps, _ = parse_otool(cur)
            for dep in deps:
                if is_system_lib(dep):
                    continue
                resolved = self.resolve_staged_dep(dep, cur)
                if resolved is None:
                    missing.append((cur, dep))
                elif resolved not in visited:
                    queue.append(resolved)

        if missing:
            formatted = "\n".join(f"  - {src.name} requires {dep}" for src, dep in missing)
            raise RuntimeError(
                "Could not resolve all arm64 MacPorts runtime dependencies:\n" + formatted
            )


def find_file_in_prefixes(dep: str, current_file: Path, prefixes: list[Path], rpaths: list[str]) -> Path | None:
    if is_system_lib(dep):
        return None
    if dep.startswith("@loader_path/"):
        rel = dep[len("@loader_path/") :]
        p = current_file.parent / rel
        if p.exists():
            return p.resolve()
        rel_name = Path(rel).name
        for prefix in prefixes:
            for pattern in (
                str(prefix / "lib" / rel_name),
                str(prefix / "opt" / "*" / "lib" / rel_name),
                str(prefix / "libexec" / "*" / "lib" / rel_name),
            ):
                hits = glob.glob(pattern)
                if hits:
                    return Path(hits[0]).resolve()
        return None
    if dep.startswith("@rpath/"):
        rel = dep[len("@rpath/") :]
        for rp in rpaths:
            for prefix in prefixes:
                if rp.startswith("/"):
                    if str(prefix) in ("/opt/local", "/usr/local", "/opt/homebrew"):
                        real_cand = Path(rp) / rel
                        if real_cand.exists():
                            return real_cand.resolve()
                    elif str(prefix) != "/":
                        stage_cand = Path(str(prefix) + rp) / rel
                        if stage_cand.exists():
                            return stage_cand.resolve()
        for prefix in prefixes:
            for pattern in (
                str(prefix / "lib" / rel),
                str(prefix / "opt" / "*" / "lib" / rel),
                str(prefix / "libexec" / "*" / "lib" / rel),
            ):
                hits = glob.glob(pattern)
                if hits:
                    return Path(hits[0]).resolve()
        return None

    # Absolute path dependency like /opt/local/lib/libfoo.dylib
    for prefix in prefixes:
        # Check prefix + relative when prefix is staged (/path/to/stage/opt/local) vs direct (/opt/local)
        if dep.startswith("/"):
            dep_name = Path(dep).name
            # If prefix ends with /opt/local (stage/opt/local) and dep starts with /opt/local
            prefix_str = str(prefix).rstrip("/")
            if prefix_str.endswith("/opt/local") and dep.startswith("/opt/local/"):
                rel_under_mp = dep[len("/opt/local/") :]
                cand = Path(prefix_str) / rel_under_mp
                if cand.exists():
                    return cand.resolve()
            cand = Path(dep) if str(prefix) == "/" else Path(dep)
            if str(prefix) in ("/opt/local", "/usr/local", "/opt/homebrew") and cand.exists():
                return cand.resolve()
            # Also search recursively inside prefix for matching filename
            hits = glob.glob(str(prefix / "**" / dep_name), recursive=True)
            if hits:
                return Path(hits[0]).resolve()
    return None


def collect_deps_for_prefix(
    seed_executables: list[Path],
    search_prefix: Path,
    required_arch: str,
) -> dict[str, Path]:
    """Return {dylib_basename: resolved_source_path} for all transitive dylibs needed in one arch."""
    collected: dict[str, Path] = {}
    queue: deque[Path] = deque(seed_executables)
    visited: set[Path] = set()

    # Always include libmpv.2.dylib even if linked lazily/indirectly
    for candidate in (
        search_prefix / "lib" / "libmpv.2.dylib",
        search_prefix / "lib" / "libmpv.dylib",
    ):
        if candidate.exists():
            resolved = candidate.resolve()
            if required_arch in lipo_archs(resolved):
                collected[resolved.name] = resolved
                queue.append(resolved)
                break

    while queue:
        cur = queue.popleft()
        if cur in visited or not cur.exists():
            continue
        visited.add(cur)
        _, deps, rpaths = parse_otool(cur)
        for dep in deps:
            if is_system_lib(dep):
                continue
            resolved = find_file_in_prefixes(dep, cur, [search_prefix], rpaths)
            if resolved is None:
                raise RuntimeError(
                    f"[{required_arch}] Unresolved dependency '{dep}' required by {cur}"
                )
            archs = lipo_archs(resolved)
            if required_arch not in archs:
                raise RuntimeError(
                    f"[{required_arch}] {resolved} lacks '{required_arch}' slice (found {archs})"
                )
            collected[resolved.name] = resolved
            if resolved not in visited:
                queue.append(resolved)

    return collected


def codesign_adhoc(target: Path) -> None:
    res = subprocess.run(
        ["codesign", "--force", "--sign", "-", str(target)],
        text=True,
        capture_output=True,
    )
    if res.returncode != 0:
        raise RuntimeError(f"codesign failed for {target}: {res.stderr.strip()}")


def verify_app_bundle(app_dir: Path, required_archs: list[str]) -> None:
    contents = app_dir / "Contents"
    frameworks = contents / "Frameworks"
    macos_dir = contents / "MacOS"
    resources_dir = contents / "Resources"

    libmpv = frameworks / "libmpv.2.dylib"
    if not libmpv.exists():
        raise RuntimeError(f"Bundle verification failed: {libmpv} is missing!")

    mpv_archs = set(lipo_archs(libmpv))
    for req in required_archs:
        if req not in mpv_archs:
            raise RuntimeError(
                f"Bundle verification failed: {libmpv} missing architecture '{req}' (found {mpv_archs})"
            )

    files_to_check: list[Path] = []
    for folder in (macos_dir, resources_dir, frameworks):
        if folder.exists():
            for p in folder.iterdir():
                if p.is_file() and not p.is_symlink() and is_macho(p):
                    files_to_check.append(p)

    for mach in files_to_check:
        _, deps, _ = parse_otool(mach)
        for dep in deps:
            if is_system_lib(dep):
                continue
            if dep.startswith("@executable_path/../Frameworks/"):
                name = dep[len("@executable_path/../Frameworks/") :]
                if not (frameworks / name).exists():
                    raise RuntimeError(f"Dangling dependency in {mach.name}: {dep}")
            elif dep.startswith("@loader_path/"):
                name = dep[len("@loader_path/") :]
                if not (mach.parent / name).exists():
                    raise RuntimeError(f"Dangling dependency in {mach.name}: {dep}")
            elif dep.startswith("@rpath/"):
                name = dep[len("@rpath/") :]
                if not (frameworks / name).exists():
                    raise RuntimeError(f"Unbundled @rpath dependency in {mach.name}: {dep}")
            else:
                raise RuntimeError(f"Non-relocatable external dependency in {mach.name}: {dep}")

    res = subprocess.run(
        ["codesign", "--verify", "--deep", "--strict", str(app_dir)],
        text=True,
        capture_output=True,
    )
    if res.returncode != 0:
        raise RuntimeError(f"codesign --verify failed for {app_dir}: {res.stderr.strip()}")

    print(
        f"[verify] OK: {app_dir.name} verified (libmpv.2.dylib archs: {sorted(mpv_archs)}, "
        f"{len(list(frameworks.glob('*.dylib')))} bundled dylibs, valid signature)"
    )


def rewrite_macho_deps(target: Path, frameworks_dir: Path, is_dylib: bool) -> None:
    install_id, deps, _ = parse_otool(target)
    if is_dylib:
        new_id = f"@rpath/{target.name}"
        if install_id != new_id:
            run_cmd(["install_name_tool", "-id", new_id, str(target)])

    for dep in deps:
        if is_system_lib(dep):
            continue
        dep_base = Path(dep).name
        if not (frameworks_dir / dep_base).exists():
            # Keep system/framework if somehow missed? Should already be bundled.
            raise RuntimeError(f"Missing bundled dylib '{dep_base}' referenced by {target}")
        new_dep = f"@loader_path/{dep_base}" if is_dylib else f"@executable_path/../Frameworks/{dep_base}"
        if dep != new_dep:
            run_cmd(["install_name_tool", "-change", dep, new_dep, str(target)])


def copy_or_lipo_resource_tool(
    tool_name: str,
    x64_candidates: list[Path],
    arm64_candidates: list[Path],
    dest: Path,
    mode: str,
) -> Path | None:
    def first_existing(cands: list[Path]) -> Path | None:
        for c in cands:
            if c.exists():
                return c.resolve()
        return None

    x64 = first_existing(x64_candidates)
    arm64 = first_existing(arm64_candidates)

    if mode == "arm64":
        if arm64:
            shutil.copy2(arm64, dest)
            os.chmod(dest, 0o755)
            return dest
        print(f"[bundle] WARN: No arm64 binary found for {tool_name}; skipping x86_64 host copy.")
    elif mode == "x86_64":
        if x64:
            shutil.copy2(x64, dest)
            os.chmod(dest, 0o755)
            return dest
    elif mode == "universal":
        if x64 and arm64 and "x86_64" in lipo_archs(x64) and "arm64" in lipo_archs(arm64):
            tmp_dir = dest.parent / ".lipo-tmp"
            tmp_dir.mkdir(exist_ok=True)
            try:
                lipo_merge_pair(x64, arm64, dest, tmp_dir)
            finally:
                shutil.rmtree(tmp_dir, ignore_errors=True)
            os.chmod(dest, 0o755)
            return dest
        src = arm64 or x64
        if src:
            shutil.copy2(src, dest)
            os.chmod(dest, 0o755)
            return dest
    return None


def cmd_stage_arm64(args: argparse.Namespace) -> None:
    host_darwin = platform.release().split(".")[0]
    darwins = [f"darwin_{host_darwin}", "darwin_24", "darwin_23", "darwin_25", "darwin_22"]
    # Preserve order without duplicates
    seen: set[str] = set()
    target_darwins = [d for d in darwins if not (d in seen or seen.add(d))]

    stager = MacPortsArm64Stager(
        macports_prefix=Path(args.macports_prefix),
        stage_dir=Path(args.stage_dir),
        cache_dir=Path(args.cache_dir),
        target_darwins=target_darwins,
    )

    seeds = ["mpv"]
    # Also try staging arm64 ffmpeg/nodejs matching the host's installed packages
    probe_bins: list[Path] = [
        Path(args.macports_prefix) / "bin" / "ffmpeg",
        Path(args.macports_prefix) / "bin" / "node",
    ]
    probe_bins.extend(
        Path(p) for p in sorted(glob.glob(f"{args.macports_prefix}/libexec/ffmpeg*/bin/ffmpeg"))
    )
    probe_bins.extend(
        Path(p) for p in sorted(glob.glob(f"{args.macports_prefix}/bin/node*"), reverse=True)
    )
    for p in probe_bins:
        if p.exists():
            port_name = stager.port_provides(str(p.resolve())) or stager.port_provides(str(p))
            if port_name and port_name not in seeds:
                seeds.append(port_name)

    stager.stage_seed_binaries_and_walk(seeds)

    lib_dir = stager.stage_path(f"{args.macports_prefix}/lib")
    libmpv = lib_dir / "libmpv.2.dylib"
    if not libmpv.exists():
        libmpv = lib_dir / "libmpv.dylib"
    if not libmpv.exists():
        raise RuntimeError(f"arm64 libmpv not found under {lib_dir}")
    print(f"[arm64-stage] Ready: {libmpv} (archs: {lipo_archs(libmpv)})")


def cmd_bundle_app(args: argparse.Namespace) -> None:
    app_dir = Path(args.app_dir)
    contents = app_dir / "Contents"
    macos_dir = contents / "MacOS"
    frameworks_dir = contents / "Frameworks"
    resources_dir = contents / "Resources"
    frameworks_dir.mkdir(parents=True, exist_ok=True)
    resources_dir.mkdir(parents=True, exist_ok=True)

    macports_prefix = Path(args.macports_prefix)
    arm64_prefix = (
        Path(args.arm64_stage) / str(macports_prefix).lstrip("/")
        if args.arm64_stage
        else macports_prefix
    )

    main_exe = macos_dir / args.exe_name
    if not main_exe.exists():
        raise RuntimeError(f"Main executable not found: {main_exe}")

    # Optionally populate Contents/Resources/{node,ffmpeg,ffprobe} from arm64 stage / host
    if args.stage_resources:
        for tool in ("node", "ffmpeg", "ffprobe"):
            x64_cands = [macports_prefix / "bin" / tool]
            arm64_cands = [arm64_prefix / "bin" / tool]
            if tool == "node":
                x64_cands.extend(Path(p) for p in sorted(glob.glob(f"{macports_prefix}/bin/node*"), reverse=True))
                arm64_cands.extend(Path(p) for p in sorted(glob.glob(f"{arm64_prefix}/bin/node*"), reverse=True))
            if tool in ("ffmpeg", "ffprobe"):
                x64_cands.extend(Path(p) for p in glob.glob(f"{macports_prefix}/libexec/ffmpeg*/bin/{tool}"))
                arm64_cands.extend(Path(p) for p in glob.glob(f"{arm64_prefix}/libexec/ffmpeg*/bin/{tool}"))
            dest = resources_dir / tool
            placed = copy_or_lipo_resource_tool(tool, x64_cands, arm64_cands, dest, args.mode)
            if placed:
                print(f"[bundle] Packaged {tool} ({lipo_archs(placed)}) -> {dest}")

    # Collect Mach-O executables inside Contents/MacOS and Contents/Resources
    executables: list[Path] = [main_exe]
    for folder in (resources_dir,):
        for p in folder.iterdir():
            if p.is_file() and not p.is_symlink() and is_macho(p):
                executables.append(p)

    # Gather transitive dependencies per required architecture
    arm64_libs: dict[str, Path] = {}
    x64_libs: dict[str, Path] = {}

    if args.mode in ("arm64", "universal"):
        arm_exes = [e for e in executables if "arm64" in lipo_archs(e)]
        arm64_libs = collect_deps_for_prefix(arm_exes, arm64_prefix, "arm64")
    if args.mode in ("x86_64", "universal"):
        x64_exes = [e for e in executables if "x86_64" in lipo_archs(e)]
        x64_libs = collect_deps_for_prefix(x64_exes, macports_prefix, "x86_64")

    all_names = sorted(set(arm64_libs.keys()) | set(x64_libs.keys()))
    tmp_lipo_dir = frameworks_dir / ".lipo-tmp"
    if args.mode == "universal":
        tmp_lipo_dir.mkdir(exist_ok=True)
    try:
        for name in all_names:
            dest = frameworks_dir / name
            if dest.exists() or dest.is_symlink():
                dest.unlink()
            a_path = arm64_libs.get(name)
            x_path = x64_libs.get(name)
            if a_path and x_path and a_path != x_path and args.mode == "universal":
                lipo_merge_pair(x_path, a_path, dest, tmp_lipo_dir)
            elif a_path:
                shutil.copy2(a_path, dest)
            elif x_path:
                shutil.copy2(x_path, dest)
            os.chmod(dest, 0o755)
    finally:
        if tmp_lipo_dir.exists():
            shutil.rmtree(tmp_lipo_dir, ignore_errors=True)

    # Ensure libmpv.dylib symlink exists if libmpv.2.dylib is bundled
    if (frameworks_dir / "libmpv.2.dylib").exists() and not (frameworks_dir / "libmpv.dylib").exists():
        (frameworks_dir / "libmpv.dylib").symlink_to("libmpv.2.dylib")

    # Rewrite install IDs and load commands
    for dylib in frameworks_dir.glob("*.dylib"):
        if not dylib.is_symlink():
            rewrite_macho_deps(dylib, frameworks_dir, is_dylib=True)

    for exe in executables:
        rewrite_macho_deps(exe, frameworks_dir, is_dylib=False)
        subprocess.run(
            ["install_name_tool", "-add_rpath", "@executable_path/../Frameworks", str(exe)],
            check=False,
            capture_output=True,
        )

    # Re-sign everything modified (mandatory for ARM64 AMFI after install_name_tool)
    for dylib in sorted(frameworks_dir.glob("*.dylib")):
        if not dylib.is_symlink():
            codesign_adhoc(dylib)
    for exe in executables:
        codesign_adhoc(exe)
    codesign_adhoc(app_dir)

    req_archs = (
        ["arm64"]
        if args.mode == "arm64"
        else (["x86_64"] if args.mode == "x86_64" else ["x86_64", "arm64"])
    )
    verify_app_bundle(app_dir, req_archs)


def main() -> int:
    parser = argparse.ArgumentParser(description="Stage & bundle macOS arm64/universal MacPorts deps")
    sub = parser.add_subparsers(dest="command", required=True)

    p_stage = sub.add_parser("stage-arm64", help="Fetch arm64 MacPorts mpv + transitive dylibs")
    p_stage.add_argument("--macports-prefix", default="/opt/local")
    p_stage.add_argument("--stage-dir", required=True)
    p_stage.add_argument("--cache-dir", required=True)

    p_bundle = sub.add_parser("bundle-app", help="Bundle & verify Frameworks inside .app")
    p_bundle.add_argument("--app-dir", required=True)
    p_bundle.add_argument("--exe-name", default="stremio-enhanced-ng")
    p_bundle.add_argument("--macports-prefix", default="/opt/local")
    p_bundle.add_argument("--arm64-stage", default="")
    p_bundle.add_argument("--mode", choices=["arm64", "x86_64", "universal"], required=True)
    p_bundle.add_argument("--stage-resources", action="store_true")

    args = parser.parse_args()
    try:
        if args.command == "stage-arm64":
            cmd_stage_arm64(args)
        elif args.command == "bundle-app":
            cmd_bundle_app(args)
    except Exception as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
