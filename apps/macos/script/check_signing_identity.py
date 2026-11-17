#!/usr/bin/env python3
"""Real signing regression on disposable App copies and a new isolated identity.

Never changes the input App, global trust, Gatekeeper, or TCC. The QA directory
contains a private isolated keychain; retain it privately with its public summary.
This verifies signing identity continuity, not macOS permission continuity.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile


class CheckError(RuntimeError):
    pass


def invoke(arguments: list[str], label: str, *, expect_success: bool = True) -> bytes:
    # Never include argv, captured diagnostics, or subprocess exceptions in errors.
    try:
        result = subprocess.run(arguments, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=180, check=False)
    except (OSError, subprocess.TimeoutExpired):
        raise CheckError(f"{label}: tool failed to run or timed out") from None
    if expect_success and result.returncode != 0:
        raise CheckError(f"{label}: operation failed (exit {result.returncode}); diagnostics withheld")
    if not expect_success and result.returncode == 0:
        raise CheckError(f"{label}: unsafe success; expected rejection")
    return result.stdout + (result.stderr if arguments[0] == "/usr/bin/codesign" else b"")


def change_info(bundle: Path, key: str, value: str) -> None:
    path = bundle / "Contents/Info.plist"
    with path.open("rb") as stream:
        info = plistlib.load(stream)
    info[key] = value
    with path.open("wb") as stream:
        plistlib.dump(info, stream, fmt=plistlib.FMT_XML, sort_keys=True)


def signed_version(signer: Path, bundle: Path, state: Path) -> dict:
    raw = invoke([sys.executable, str(signer), str(bundle), "--state-dir", str(state)], "Sign isolated App")
    try:
        receipt = json.loads(raw)
    except (ValueError, TypeError):
        raise CheckError("Signing receipt is not valid JSON") from None
    if receipt.get("strict_verification") != "passed":
        raise CheckError("Signing receipt did not confirm strict verification")
    invoke(["/usr/bin/codesign", "--verify", "--deep", "--strict", "--verbose=2", str(bundle)],
           "Independently verify isolated App")
    display = invoke(["/usr/bin/codesign", "-d", "-r-", "--verbose=4", str(bundle)], "Inspect isolated App")
    hashes = re.findall(rb"^CDHash=([0-9a-fA-F]+)\s*$", display, re.MULTILINE)
    requirements = re.findall(rb"^\s*#?\s*designated\s*=>\s*(.+)$", display, re.MULTILINE)
    if len(hashes) != 1 or len(requirements) != 1:
        raise CheckError("Could not uniquely inspect code hash and designated requirement")
    requirement = " ".join(requirements[0].decode().split())
    if requirement != receipt.get("designated_requirement"):
        raise CheckError("Observed designated requirement differs from the signing receipt")
    # Whitelist public fields; never serialize signing state or arbitrary output.
    return {"cdhash": hashes[0].decode().lower(), "designated_requirement": requirement,
            "certificate_sha1": receipt["certificate_sha1"],
            "certificate_sha256": receipt["certificate_sha256"], "strict_verification": "passed"}


def snapshot(state: Path) -> dict[str, str]:
    # Hashes are used only in memory for equality checks and never reported.
    result = {str(path.relative_to(state)): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in state.rglob("*") if path.is_file()}
    witness = state.parent / ("." + state.name + ".identity-established.json")
    result["external-witness"] = hashlib.sha256(witness.read_bytes()).hexdigest()
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-bundle", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    if sys.platform != "darwin":
        raise CheckError("This regression requires macOS")
    source = args.app_bundle.expanduser().resolve()
    if not (source / "Contents/Info.plist").is_file():
        raise CheckError("Input App bundle is missing Info.plist")
    # This small SwiftPM App needs no symlinks. Refuse them rather than risk a
    # copied executable/resource link allowing codesign to touch original files.
    if any(path.is_symlink() for path in source.rglob("*")):
        raise CheckError("This isolated regression requires an App without symbolic links")
    signer = Path(__file__).resolve().with_name("local_signing.py")
    if not signer.is_file():
        raise CheckError("Local signing script is missing")
    os.umask(0o077)
    output = args.output_dir.expanduser().resolve()
    if output == source or source in output.parents:
        raise CheckError("QA output directory must be outside the input App")
    output.mkdir(parents=True, exist_ok=True)
    qa = Path(tempfile.mkdtemp(prefix="signing-identity-", dir=output))
    os.chmod(qa, 0o700)
    state = qa / "Signing"
    version_a, version_b = qa / "VersionA.app", qa / "VersionB.app"
    shutil.copytree(source, version_a, symlinks=True)
    # Info.plist must be a local file: avoid following a copied external symlink.
    if (version_a / "Contents/Info.plist").is_symlink():
        raise CheckError("Input Info.plist must not be a symbolic link")
    a = signed_version(signer, version_a, state)
    shutil.copytree(version_a, version_b, symlinks=True)
    with (version_b / "Contents/Info.plist").open("rb") as stream:
        initial_version = str(plistlib.load(stream).get("CFBundleVersion", "0"))
    change_info(version_b, "CFBundleVersion", initial_version + ".identity-regression")
    b = signed_version(signer, version_b, state)
    if a["cdhash"] == b["cdhash"]:
        raise CheckError("Version change did not change the code hash")
    for key in ("designated_requirement", "certificate_sha1", "certificate_sha256"):
        if a[key] != b[key]:
            raise CheckError("Signing identity changed across the two App versions")
    # Recheck both retained artifacts after signing B with the same identity.
    for bundle in (version_a, version_b):
        invoke(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(bundle)], "Reverify retained App")

    before = snapshot(state)
    keychain = state / "development.keychain-db"
    held = state / "development.keychain-db.qa-held"
    keychain.rename(held)
    try:
        invoke([sys.executable, str(signer), str(version_b), "--state-dir", str(state)],
               "Reject missing original keychain", expect_success=False)
        if keychain.exists():
            raise CheckError("Missing keychain was unexpectedly recreated")
    finally:
        if held.exists():
            if keychain.exists():
                raise CheckError("Unexpected replacement keychain; original retained in QA directory")
            held.rename(keychain)
    if snapshot(state) != before:
        raise CheckError("Identity state changed after a rejected missing-keychain attempt")

    invalid = qa / "WrongIdentifier.app"
    shutil.copytree(version_b, invalid, symlinks=True)
    change_info(invalid, "CFBundleIdentifier", "invalid.candid-oss.signing-regression")
    invalid_state = qa / "InvalidIdentifierSigning"
    invoke([sys.executable, str(signer), str(invalid), "--state-dir", str(invalid_state)],
           "Reject wrong bundle identifier", expect_success=False)
    if invalid_state.exists() or (qa / ".InvalidIdentifierSigning.identity-established.json").exists():
        raise CheckError("Wrong bundle identifier unexpectedly created identity state")

    summary = {"scope": "isolated_real_signing_only", "qa_directory": str(qa),
               "version_a": a, "version_b": b, "code_hash_changed": True,
               "designated_requirement_stable": True, "certificate_stable": True,
               "missing_keychain_rejected_without_identity_recreation": True,
               "wrong_bundle_rejected_before_state_creation": True,
               "permission_continuity_tested": False, "result": "passed"}
    summary_path = qa / "summary.json"
    summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    os.chmod(summary_path, 0o600)
    print(json.dumps(summary, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except CheckError as error:
        print("Signing regression stopped: " + str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError, KeyError, TypeError):
        print("Signing regression stopped: isolated local files could not be processed; diagnostics withheld.", file=sys.stderr)
        sys.exit(1)
