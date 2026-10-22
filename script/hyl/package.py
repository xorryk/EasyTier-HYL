"""Package tested HYL binaries without including machine-specific configuration."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[2]
NIGHTLY = "99db92a3aeb3109efe1ace5a4a3e2a1bf3840ed7"
PATCH = "6649c35aea3769a7f1704a4ae1fe564e4f2077a9"
BASELINE = "1624c81d9f47e841fcb6828958874037194eb476"
VALIDATION_PATCH = "4c0a313ad15d0d12c91999648c96cdb489dd7c1b"
RECONNECT_BASELINE = "e7f8f27ed561457c52912395f0df625377fe198d"
VERSION_MARKER = "-hyl.5-test.2"
PLATFORMS = ("windows-x86_64", "macos-aarch64")


def output(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, timeout=30).strip()


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def package(args):
    release_dir = ROOT / "target" / args.target / "release"
    destination = ROOT / "dist" / "hyl"
    destination.mkdir(parents=True, exist_ok=True)
    suffix = ".exe" if args.platform.startswith("windows") else ""
    core = release_dir / ("easytier-core" + suffix)
    cli = release_dir / ("easytier-cli" + suffix)
    version = output(str(core), "--version")
    if VERSION_MARKER not in version:
        raise RuntimeError("Core binary is missing the HYL version marker")
    cli_version = output(str(cli), "--version")
    if VERSION_MARKER not in cli_version:
        raise RuntimeError("CLI binary is missing the HYL version marker")

    prefix = f"EasyTier-HYL-{args.release}-{args.platform}"
    archive = destination / f"{prefix}-core.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
        for binary in (core, cli):
            bundle.write(binary, binary.name)
        for filename in ("LICENSE", "HYL-BUILD.md", "HYL-VALIDATION.md"):
            bundle.write(ROOT / filename, filename)
        if suffix:
            drivers = ROOT / "easytier" / "third_party" / "x86_64"
            for required in ("WinDivert64.sys", "wintun.dll", "Packet.dll"):
                if not (drivers / required).is_file():
                    raise RuntimeError(f"Missing Windows dependency: {required}")
            for dependency in sorted(drivers.iterdir()):
                if dependency.suffix.lower() in (".dll", ".sys"):
                    bundle.write(dependency, dependency.name)

    pattern = "bundle/nsis/*.exe" if suffix else "bundle/dmg/*.dmg"
    installers = list(release_dir.glob(pattern))
    if len(installers) != 1:
        raise RuntimeError(f"Expected one GUI installer, found {installers}")
    installer = destination / (f"{prefix}-setup.exe" if suffix else f"{prefix}.dmg")
    shutil.copy2(installers[0], installer)

    metadata = {
        "release": args.release,
        "platform": args.platform,
        "target": args.target,
        "upstream_nightly": NIGHTLY,
        "upstream_pr": 2256,
        "upstream_patch": PATCH,
        "baseline_hyl_commit": BASELINE,
        "validation_pr": 2243,
        "validation_patch": VALIDATION_PATCH,
        "reconnect_baseline": RECONNECT_BASELINE,
        "channel": "validation-only",
        "source_commit": output("git", "rev-parse", "HEAD"),
        "core_version": version,
        "cli_version": cli_version,
        "rustc": output("rustc", "--version"),
        "github_run_id": os.environ.get("GITHUB_RUN_ID"),
        "signing": "unsigned" if suffix else "ad-hoc; not Apple notarized",
        "validation": (
            "IPv4 and IPv6 connector socket selection tests passed; "
            "address-scoped listener capture, IPv6 renumbering payload delivery, reconnect, live-old-address preservation and stale capture cleanup regression tests passed; "
            "FakeTCP ACK/SACK, handshake-state and zero-fill regression tests passed; "
            "IPv4/IPv6 FakeTCP-to-framed-reader continuity and real reset tests passed; "
            "framed transport invalid-packet rejection tests passed; "
            + ("Darwin BPF ABI, batched parsing and configurable native buffer tests passed" if not suffix else "BPF setting is macOS-only")
        ),
        "network_validation": "NOT YET VERIFIED: real IPv6 renumbering, reconnect and Windows-to-Mac streaming must be compared with hyl.5-test.1",
        "files": {file.name: sha256(file) for file in (archive, installer)},
    }
    manifest = destination / f"BUILD-INFO-{args.platform}.json"
    manifest.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(metadata, indent=2))


def verify(directory):
    directory = directory.resolve()
    source_commits = set()
    releases = set()
    for platform in PLATFORMS:
        metadata = json.loads((directory / f"BUILD-INFO-{platform}.json").read_text(encoding="utf-8"))
        if metadata["platform"] != platform or metadata["upstream_nightly"] != NIGHTLY:
            raise RuntimeError(f"Unexpected build provenance for {platform}")
        if metadata["upstream_patch"] != PATCH or VERSION_MARKER not in metadata["core_version"] or VERSION_MARKER not in metadata["cli_version"]:
            raise RuntimeError(f"Missing patch or version marker for {platform}")
        if (metadata.get("baseline_hyl_commit") != BASELINE
                or metadata.get("validation_patch") != VALIDATION_PATCH
                or metadata.get("validation_pr") != 2243
                or metadata.get("reconnect_baseline") != RECONNECT_BASELINE
                or metadata.get("channel") != "validation-only"):
            raise RuntimeError(f"Missing validation provenance for {platform}")
        source_commits.add(metadata["source_commit"])
        releases.add(metadata["release"])
        if len(metadata["files"]) != 2:
            raise RuntimeError(f"Incomplete build for {platform}")
        for name, expected in metadata["files"].items():
            artifact = directory / name
            if artifact.parent != directory or not artifact.is_file() or sha256(artifact) != expected:
                raise RuntimeError(f"Artifact missing or checksum mismatch: {name}")
    if len(source_commits) != 1 or len(releases) != 1:
        raise RuntimeError("Platform builds do not match the same source/release")
    expected_source = os.environ.get("GITHUB_SHA")
    if expected_source and source_commits != {expected_source}:
        raise RuntimeError("Artifacts do not match the workflow source commit")
    print("Verified two platform builds, four packages, and all recorded SHA-256 hashes.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target")
    parser.add_argument("--platform", choices=PLATFORMS)
    parser.add_argument("--release")
    parser.add_argument("--verify-directory", type=Path)
    args = parser.parse_args()
    if args.verify_directory:
        verify(args.verify_directory)
    elif all((args.target, args.platform, args.release)):
        package(args)
    else:
        parser.error("Provide --target, --platform and --release, or --verify-directory")


if __name__ == "__main__":
    main()
