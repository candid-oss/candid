#!/usr/bin/env python3
"""Stable, local-only signing for io.github.candid-oss.candid.

Uses an isolated keychain and an explicit certificate identity; never changes
System/Login trust, Gatekeeper, TCC, default keychains, or code requirements.
Apple references: TN2206 (self-signed identities / explicit keychain signing),
TN3127 (designated requirements). Local signing is not Developer ID/notarization.

First initialization is recorded before key generation. Interrupted, missing,
expired, or mismatched identity state fails closed; no automatic key rotation.
--state-dir is for isolated QA only. Do not delete identity state to fix a build.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import secrets
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile

BUNDLE_ID = "io.github.candid-oss.candid"
SECURITY = "/usr/bin/security"
CODESIGN = "/usr/bin/codesign"
DEFAULT_STATE = Path.home() / "Library/Application Support/Candid/Signing"
STATE_VERSION = 1


class SigningError(RuntimeError):
    pass


def run(arguments: list[str], label: str, *, fd_passwords: dict[str, str] | None = None) -> bytes:
    """Never log argv, subprocess output, exceptions containing argv, or secrets.

    security requires passwords in argv; these short-lived arguments are never
    interpolated through a shell or printed. OpenSSL passwords use anonymous FDs,
    so temporary private-key encryption passwords are never stored on disk.
    """
    opened: list[int] = []
    command = list(arguments)
    try:
        for token, password in (fd_passwords or {}).items():
            reader, writer = os.pipe()
            opened.append(reader)
            try:
                os.write(writer, (password + "\n").encode())
            finally:
                os.close(writer)
            command = [f"fd:{reader}" if item == f"fd:{token}" else item for item in command]
        try:
            result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, pass_fds=tuple(opened), timeout=120, check=False)
        except subprocess.TimeoutExpired:
            raise SigningError(f"{label}: timed out; no identity or permission fallback was attempted") from None
        except OSError:
            raise SigningError(f"{label}: could not launch the required system tool") from None
        if result.returncode:
            diagnostic = result.stderr.decode(errors="replace").casefold()
            reason = "required operation failed"
            if any(item in diagnostic for item in ("not trusted", "unable to build chain", "invalid trust")):
                reason = "certificate trust policy rejected this identity; global trust will not be changed"
            elif any(item in diagnostic for item in ("interaction is not allowed", "interaction required", "errsecinteractionnotallowed")):
                reason = "key access needs user interaction; the private-key ACL will not be broadened"
            elif "errsecinternalcomponent" in diagnostic:
                reason = "Security framework rejected signing/key access; no trust or ACL workaround was attempted"
            raise SigningError(f"{label}: {reason} (exit {result.returncode})") from None
        return result.stdout + (result.stderr if arguments[0] == CODESIGN else b"")
    finally:
        for descriptor in opened:
            os.close(descriptor)


def private_file(path: Path, *, required: bool = True) -> None:
    try:
        details = path.lstat()
    except FileNotFoundError:
        if required:
            raise SigningError("Signing identity files are missing; restore the original identity, do not regenerate") from None
        return
    if not stat.S_ISREG(details.st_mode) or details.st_uid != os.getuid():
        raise SigningError("Signing state contains an unexpected file type or owner")
    if stat.S_IMODE(details.st_mode) & 0o077:
        raise SigningError("Signing state file permissions must be 0600 or stricter")


def write_private(path: Path, data: bytes, *, new: bool = False) -> None:
    if new:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags, 0o600)
        with os.fdopen(descriptor, "wb") as file:
            file.write(data); file.flush(); os.fsync(file.fileno())
        return
    with tempfile.NamedTemporaryFile(prefix=".signing-state-", dir=path.parent, delete=False) as file:
        temporary = Path(file.name)
        os.fchmod(file.fileno(), 0o600)
        file.write(data); file.flush(); os.fsync(file.fileno())
    try:
        if path.exists():
            private_file(path)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def write_json(path: Path, value: dict, *, new: bool = False) -> None:
    write_private(path, (json.dumps(value, indent=2, sort_keys=True) + "\n").encode(), new=new)


def read_json(path: Path) -> dict:
    private_file(path)
    try:
        result = json.loads(path.read_bytes())
    except (OSError, ValueError):
        raise SigningError("Signing identity metadata is unreadable; restore the original state") from None
    if not isinstance(result, dict):
        raise SigningError("Signing identity metadata has an invalid format")
    return result


def keychain_search_list() -> list[str]:
    try:
        return shlex.split(run([SECURITY, "list-keychains", "-d", "user"], "Read user keychain search list").decode())
    except ValueError:
        raise SigningError("User keychain search list could not be parsed safely") from None


def detach_only_own_keychain(keychain: Path) -> None:
    """Remove only our path from a freshly read list, never restore an old snapshot.

    The security CLI has no compare-and-swap for its per-user list. Re-read
    immediately before mutation, abort on an observed race, and verify afterward.
    This does not claim atomicity against another program changing the list in
    the final read/write interval; callers must serialize keychain setup work.
    """
    own = str(keychain.resolve())
    for _ in range(3):
        current = keychain_search_list()
        remaining = [item for item in current if str(Path(item).expanduser().resolve()) != own]
        if remaining == current:
            return
        if keychain_search_list() != current:
            continue
        run([SECURITY, "list-keychains", "-d", "user", "-s", *remaining], "Detach isolated signing keychain")
        after = keychain_search_list()
        if after != remaining:
            raise SigningError("Concurrent keychain search-list change detected; no old list was restored")
        return
    raise SigningError("Concurrent keychain search-list changes prevent safe setup; no old list was restored")


@contextmanager
def signing_lock(state: Path):
    state.parent.mkdir(parents=True, exist_ok=True)
    lock = state.parent / ("." + state.name + ".signing.lock")
    descriptor = os.open(lock, os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        details = os.fstat(descriptor)
        if details.st_uid != os.getuid() or not stat.S_ISREG(details.st_mode) or stat.S_IMODE(details.st_mode) & 0o077:
            raise SigningError("Signing coordination file has unexpected ownership or permissions")
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def certificate_identity(openssl: str, certificate: Path) -> dict[str, str]:
    private_file(certificate)
    run([openssl, "x509", "-in", str(certificate), "-noout", "-checkend", "0"], "Check existing certificate validity")
    der = run([openssl, "x509", "-in", str(certificate), "-outform", "DER"], "Read public signing certificate")
    return {"certificate_sha1": hashlib.sha1(der).hexdigest().upper(),
            "certificate_sha256": hashlib.sha256(der).hexdigest()}


def initialize(state: Path, witness: Path, openssl: str) -> dict:
    if witness.exists():
        raise SigningError("Signing state was previously initialized or initialization was interrupted; restore the original identity")
    if state.exists():
        details = state.lstat()
        if not stat.S_ISDIR(details.st_mode) or details.st_uid != os.getuid() or stat.S_IMODE(details.st_mode) != 0o700:
            raise SigningError("Signing directory must be owned by this user with mode 0700")
        if any(state.iterdir()):
            raise SigningError("Existing incomplete signing state will not be replaced or regenerated")
    else:
        state.mkdir(mode=0o700)
    created = datetime.datetime.now(datetime.timezone.utc).isoformat()
    origin = {"state_version": STATE_VERSION, "bundle_id": BUNDLE_ID, "state_dir": str(state),
              "created_at_utc": created, "status": "initializing"}
    # Witness outside state survives accidental deletion of Signing itself.
    write_json(witness, origin, new=True)
    write_json(state / "initialization.json", origin, new=True)
    password = secrets.token_urlsafe(48)
    write_private(state / "keychain-password", password.encode(), new=True)
    keychain = state / "development.keychain-db"
    certificate = state / "identity.pem"
    created_keychain = False
    try:
        with tempfile.TemporaryDirectory(prefix=".identity-init-", dir=state) as temporary:
            directory = Path(temporary)
            os.chmod(directory, 0o700)
            key = directory / "private-key-encrypted.pem"
            generated_certificate = directory / "identity.pem"
            archive = directory / "identity.p12"
            config = directory / "openssl.cnf"
            common_name = "Candid Local Development " + secrets.token_hex(8)
            config.write_text("[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=codesign\n"
                              f"[dn]\nCN={common_name}\nO=Candid\n"
                              "[codesign]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n"
                              "extendedKeyUsage=critical,codeSigning\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid:always\n")
            key_password, archive_password = secrets.token_urlsafe(48), secrets.token_urlsafe(48)
            run([openssl, "req", "-new", "-x509", "-newkey", "rsa:3072", "-sha256", "-days", "3650",
                 "-config", str(config), "-keyout", str(key), "-out", str(generated_certificate),
                 "-passout", "fd:KEY"], "Generate temporary encrypted local signing key", fd_passwords={"KEY": key_password})
            os.chmod(key, 0o600); os.chmod(generated_certificate, 0o600)
            run([openssl, "pkcs12", "-export", "-inkey", str(key), "-in", str(generated_certificate),
                 "-out", str(archive), "-passin", "fd:KEY", "-passout", "fd:ARCHIVE"],
                "Prepare temporary encrypted identity import", fd_passwords={"KEY": key_password, "ARCHIVE": archive_password})
            os.chmod(archive, 0o600)
            run([SECURITY, "create-keychain", "-p", password, str(keychain)], "Create isolated signing keychain")
            created_keychain = True
            os.chmod(keychain, 0o600)
            # Only codesign may use the imported private key; no all-apps ACL or partition broadening.
            run([SECURITY, "import", str(archive), "-k", str(keychain), "-f", "pkcs12", "-P", archive_password,
                 "-x", "-T", CODESIGN], "Import non-extractable signing key for codesign only")
            run([SECURITY, "set-keychain-settings", "-l", "-u", "-t", "120", str(keychain)], "Set isolated keychain locking")
            write_private(certificate, generated_certificate.read_bytes(), new=True)
            identity = {**origin, **certificate_identity(openssl, certificate), "status": "ready",
                        "common_name": common_name, "designated_requirement": None}
            write_json(state / "identity.json", identity, new=True)
            write_json(witness, identity)
            return identity
    finally:
        if created_keychain:
            try:
                run([SECURITY, "lock-keychain", str(keychain)], "Lock initialized signing keychain")
            finally:
                detach_only_own_keychain(keychain)


def load_identity(state: Path, witness: Path, openssl: str) -> dict:
    if not (state / "identity.json").exists():
        return initialize(state, witness, openssl)
    details = state.lstat()
    if not stat.S_ISDIR(details.st_mode) or details.st_uid != os.getuid() or stat.S_IMODE(details.st_mode) != 0o700:
        raise SigningError("Signing directory must be owned by this user with mode 0700")
    identity = read_json(state / "identity.json")
    outside = read_json(witness)
    for key in ("state_version", "bundle_id", "state_dir", "certificate_sha1", "certificate_sha256", "status", "designated_requirement"):
        if identity.get(key) != outside.get(key):
            raise SigningError("Signing identity and durable witness disagree; no replacement identity will be created")
    if identity.get("status") != "ready" or identity.get("state_version") != STATE_VERSION or identity.get("bundle_id") != BUNDLE_ID or identity.get("state_dir") != str(state):
        raise SigningError("Signing identity metadata is not valid for this bundle and directory")
    private_file(state / "development.keychain-db"); private_file(state / "keychain-password")
    actual = certificate_identity(openssl, state / "identity.pem")
    if any(identity.get(key) != value for key, value in actual.items()):
        raise SigningError("Public certificate identity changed; restore the original certificate/keychain")
    return identity


def designated_requirement(display: bytes, sha1: str) -> str:
    text = display.decode(errors="replace")
    if "signature=adhoc" in text.casefold() or re.search(r"flags=.*\badhoc\b", text, re.IGNORECASE):
        raise SigningError("Ad-hoc signing is forbidden for persistent permissions")
    matches = re.findall(r"^\s*#?\s*designated\s*=>\s*(.+)$", text, re.MULTILINE)
    if len(matches) != 1:
        raise SigningError("A unique designated requirement could not be inspected")
    value = " ".join(matches[0].split())
    # Accept the normal synthesized two-part DR, in either term order. Fail on
    # unfamiliar/wider requirements instead of injecting a weaker replacement.
    terms = [term.strip().strip("()").strip() for term in re.split(r"\s+and\s+", value)]
    expected_identifier = f'identifier "{BUNDLE_ID}"'
    anchors = [term for term in terms if re.fullmatch(r'(?:anchor|certificate\s+root\s*=)\s+H"[0-9a-fA-F]{40}"', term)]
    if len(terms) != 2 or expected_identifier not in terms or len(anchors) != 1 or anchors[0].split('"')[1].upper() != sha1:
        raise SigningError("Designated requirement is not the expected identifier plus pinned certificate anchor")
    if "cdhash" in value.casefold():
        raise SigningError("Code-hash-bound designated requirements are forbidden")
    return value


def sign(bundle: Path, state: Path, openssl: str) -> dict:
    witness = state.parent / ("." + state.name + ".identity-established.json")
    with signing_lock(state):
        identity = load_identity(state, witness, openssl)
        keychain = state / "development.keychain-db"
        password = (state / "keychain-password").read_text()
        if len(password) < 32 or "\n" in password:
            raise SigningError("Stored signing keychain password has an invalid format")
        unlocked = False
        try:
            run([SECURITY, "unlock-keychain", "-p", password, str(keychain)], "Unlock isolated signing keychain")
            unlocked = True
            identities = run([SECURITY, "find-identity", "-p", "codesigning", str(keychain)], "Locate existing private-key identity")
            if identity["certificate_sha1"].encode() not in identities.upper():
                raise SigningError("Original private-key identity is unavailable; no key rotation or trust change was attempted")
            # Nested extensions first, with the same fixed certificate and their own
            # sandbox entitlements; the app signature then seals them.
            plugins = bundle / "Contents/PlugIns"
            for nested in sorted(plugins.glob("*.appex")) if plugins.is_dir() else []:
                with (nested / "Contents/Info.plist").open("rb") as file:
                    nested_id = plistlib.load(file).get("CFBundleIdentifier", "")
                entitlements = Path(__file__).resolve().parent / (nested.stem + ".entitlements")
                if not nested_id.startswith(BUNDLE_ID + ".") or not entitlements.is_file():
                    raise SigningError("Nested extension identity or entitlements are missing: " + nested.name)
                run([CODESIGN, "--force", "--sign", identity["certificate_sha1"], "--keychain", str(keychain),
                     "--identifier", nested_id, "--entitlements", str(entitlements), "--timestamp=none", str(nested)],
                    "Sign nested extension " + nested.name)
            # Helpers (the Rust core `candid`: the App's helper process and the command line) are signed with the
            # hardened runtime before the app seals them.
            helpers = bundle / "Contents/Helpers"
            for tool in sorted(helpers.iterdir()) if helpers.is_dir() else []:
                if tool.is_file() and os.access(tool, os.X_OK):
                    run([CODESIGN, "--force", "--sign", identity["certificate_sha1"], "--keychain", str(keychain),
                         "--identifier", BUNDLE_ID + "." + tool.name, "--options", "runtime", "--timestamp=none", str(tool)],
                        "Sign helper " + tool.name)
            run([CODESIGN, "--force", "--sign", identity["certificate_sha1"], "--keychain", str(keychain),
                 "--identifier", BUNDLE_ID, "--timestamp=none", str(bundle)], "Sign with fixed local certificate")
            run([CODESIGN, "--verify", "--deep", "--strict", "--verbose=2", str(bundle)], "Strictly verify signed bundle")
            display = run([CODESIGN, "-d", "-r-", "--verbose=4", str(bundle)], "Inspect stable designated requirement")
            requirement = designated_requirement(display, identity["certificate_sha1"])
            previous = identity.get("designated_requirement")
            if previous is not None and requirement != previous:
                raise SigningError("Designated requirement changed across builds; original identity state was preserved")
            with tempfile.TemporaryDirectory(prefix=".public-chain-", dir=state) as temporary:
                prefix = Path(temporary) / "certificate"
                run([CODESIGN, "-d", "--extract-certificates=" + str(prefix), str(bundle)], "Inspect actual signing certificate")
                leaf = Path(str(prefix) + "0")
                if not leaf.is_file() or hashlib.sha256(leaf.read_bytes()).hexdigest() != identity["certificate_sha256"]:
                    raise SigningError("Signed bundle certificate does not match the fixed public identity")
            if previous is None:
                identity["designated_requirement"] = requirement
                write_json(state / "identity.json", identity)
                write_json(witness, identity)
            return {"bundle_id": BUNDLE_ID, "certificate_sha1": identity["certificate_sha1"],
                    "certificate_sha256": identity["certificate_sha256"], "designated_requirement": requirement,
                    "signing": "persistent_local_self_signed", "strict_verification": "passed"}
        finally:
            if unlocked:
                run([SECURITY, "lock-keychain", str(keychain)], "Lock signing keychain after signing")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app_bundle", type=Path)
    parser.add_argument("--state-dir", type=Path, default=DEFAULT_STATE, help="Isolated QA override; production uses the default persistent state")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("local signing requires macOS")
    bundle = args.app_bundle.expanduser().resolve()
    state_input = args.state_dir.expanduser().absolute()
    if state_input.is_symlink():
        raise SigningError("Signing state directory must not be a symbolic link")
    state = state_input.resolve()
    try:
        with (bundle / "Contents/Info.plist").open("rb") as file:
            info = plistlib.load(file)
    except (OSError, ValueError, plistlib.InvalidFileException):
        raise SigningError("App bundle Info.plist is missing or invalid") from None
    if info.get("CFBundleIdentifier") != BUNDLE_ID or info.get("CFBundleExecutable") != "Candid" or not (bundle / "Contents/MacOS/Candid").is_file():
        raise SigningError("Only the fixed io.github.candid-oss.candid / Candid App bundle may be signed")
    openssl = shutil.which("openssl")
    if not openssl:
        raise SigningError("OpenSSL is required in PATH; no system or trust setting was changed")
    os.umask(0o077)
    result = sign(bundle, state, openssl)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except SigningError as error:
        print("Local signing stopped: " + str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError, KeyError):
        # A traceback could expose subprocess arguments or private file contents.
        print("Local signing stopped: local state could not be processed; restore or inspect the original identity state.", file=sys.stderr)
        sys.exit(1)
