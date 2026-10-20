#!/usr/bin/env python3
"""Version and release notes for a release.

Cabin keeps its version in four places, and they must agree:

- Cargo.toml, the version in [workspace.package]. The About dialogue shows it.
- meson.build, the project version.
- Cargo.lock, the version of each workspace crate.
- The metainfo file, the newest <release> entry.

cargo-sources.json lists only the crates that come from crates.io, so a version
change does not touch it. If it ever lists a workspace crate, "bump" runs
"just flatpak-sources".

release.py version
    Prints the version. Fails unless Cargo.toml, meson.build and Cargo.lock agree.
release.py bump VERSION --note TEXT [--note TEXT ...] [--date YYYY-MM-DD]
    Sets VERSION in Cargo.toml, meson.build and Cargo.lock, and adds a <release>
    entry at the top of the metainfo file. One note becomes a paragraph, more
    notes become a list.
release.py date VERSION [--date YYYY-MM-DD]
    Sets the date of the metainfo entry of VERSION. The default is today (UTC),
    also for "bump".
release.py notes VERSION
    Prints the notes of VERSION from the metainfo file as Markdown.
release.py check TAG
    Fails unless TAG is "v" + the version and the newest metainfo entry is that
    version.
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path
from xml.sax.saxutils import escape

SOURCE_DIR = Path(__file__).resolve().parents[2]
CARGO_TOML = SOURCE_DIR / "Cargo.toml"
CARGO_LOCK = SOURCE_DIR / "Cargo.lock"
CARGO_SOURCES = SOURCE_DIR / "cargo-sources.json"
MESON = SOURCE_DIR / "meson.build"
METAINFO = SOURCE_DIR / "data" / "cabin.metainfo.xml.in.in"

_VERSION = re.compile(r"^\d+\.\d+\.\d+$")
_DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
# The version line in [workspace.package]. The section ends at the next header.
_CARGO_VERSION = re.compile(
    r'(^\[workspace\.package\]\n(?:(?!\[).*\n)*?version = ")([^"]+)(")', re.MULTILINE
)
_MESON_VERSION = re.compile(r"(^project\((?:.*\n)*?\s*version: ')([^']+)(')", re.MULTILINE)


class ReleaseError(Exception):
    pass


def parse_version(version: str) -> tuple[int, ...]:
    if not _VERSION.match(version):
        raise ReleaseError(f"{version} is not a version such as 1.2.3.")
    return tuple(int(part) for part in version.split("."))


def _search(pattern: re.Pattern[str], text: str, name: str) -> re.Match[str]:
    match = pattern.search(text)
    if not match:
        raise ReleaseError(f"{name} has no version line.")
    return match


def cargo_version(cargo_toml: str) -> str:
    return _search(_CARGO_VERSION, cargo_toml, "Cargo.toml")[2]


def meson_version(meson: str) -> str:
    return _search(_MESON_VERSION, meson, "meson.build")[2]


def set_cargo_version(cargo_toml: str, version: str) -> str:
    _search(_CARGO_VERSION, cargo_toml, "Cargo.toml")
    return _CARGO_VERSION.sub(rf"\g<1>{version}\g<3>", cargo_toml, count=1)


def set_meson_version(meson: str, version: str) -> str:
    _search(_MESON_VERSION, meson, "meson.build")
    return _MESON_VERSION.sub(rf"\g<1>{version}\g<3>", meson, count=1)


def workspace_crates(cargo_toml: str) -> list[str]:
    members = tomllib.loads(cargo_toml)["workspace"]["members"]
    names = []
    for member in members:
        manifest = tomllib.loads((SOURCE_DIR / member / "Cargo.toml").read_text())
        names.append(manifest["package"]["name"])
    return names


def _lock_blocks(lock: str) -> list[str]:
    # Each block starts with "[[package]]". The text before the first one is the header.
    return re.split(r"(?=^\[\[package\]\]\n)", lock, flags=re.MULTILINE)


def _is_workspace_block(block: str, crates: list[str]) -> bool:
    # A workspace crate has no "source" line. A crates.io crate with the same name has one.
    name = re.search(r'^name = "([^"]+)"$', block, re.MULTILINE)
    return bool(name) and name[1] in crates and not re.search(r"^source = ", block, re.MULTILINE)


def lock_versions(lock: str, crates: list[str]) -> dict[str, str]:
    versions = {}
    for block in _lock_blocks(lock):
        if _is_workspace_block(block, crates):
            name = re.search(r'^name = "([^"]+)"$', block, re.MULTILINE)[1]
            versions[name] = re.search(r'^version = "([^"]+)"$', block, re.MULTILINE)[1]
    missing = set(crates) - set(versions)
    if missing:
        raise ReleaseError(f"Cargo.lock has no entry for {', '.join(sorted(missing))}.")
    return versions


def set_lock_version(lock: str, crates: list[str], version: str) -> str:
    blocks = _lock_blocks(lock)
    for index, block in enumerate(blocks):
        if _is_workspace_block(block, crates):
            blocks[index] = re.sub(
                r'^version = "[^"]+"$',
                f'version = "{version}"',
                block,
                count=1,
                flags=re.MULTILINE,
            )
    return "".join(blocks)


def vendors_workspace_crate(cargo_sources: str, crates: list[str]) -> bool:
    dests = {entry.get("dest", "") for entry in json.loads(cargo_sources)}
    return any(dest.startswith(f"cargo/vendor/{crate}-") for dest in dests for crate in crates)


def current_version() -> str:
    cargo_toml = CARGO_TOML.read_text()
    version = cargo_version(cargo_toml)
    meson = meson_version(MESON.read_text())
    if meson != version:
        raise ReleaseError(f"meson.build has version {meson}, but Cargo.toml has {version}.")
    for crate, locked in lock_versions(
        CARGO_LOCK.read_text(), workspace_crates(cargo_toml)
    ).items():
        if locked != version:
            raise ReleaseError(f"Cargo.lock has {crate} {locked}, but Cargo.toml has {version}.")
    return version


def release_versions(metainfo: str) -> list[str]:
    root = ET.fromstring(metainfo)
    return [release.get("version", "") for release in root.iter("release")]


def release_entry(version: str, date: str, notes: list[str]) -> str:
    if not notes:
        raise ReleaseError("Give at least one note.")
    if len(notes) == 1:
        body = f"        <p>{escape(notes[0])}</p>\n"
    else:
        items = "".join(f"          <li>{escape(note)}</li>\n" for note in notes)
        body = f"        <ul>\n{items}        </ul>\n"
    return (
        f'    <release version="{version}" date="{date}">\n'
        f"      <description>\n{body}      </description>\n"
        f"    </release>\n"
    )


def add_release(metainfo: str, version: str, date: str, notes: list[str]) -> str:
    versions = release_versions(metainfo)
    if version in versions:
        raise ReleaseError(f"The metainfo already has release {version}.")
    if versions and parse_version(version) <= parse_version(versions[0]):
        raise ReleaseError(f"{version} is not newer than {versions[0]}.")
    marker = "  <releases>\n"
    if marker not in metainfo:
        raise ReleaseError("The metainfo has no <releases> element.")
    return metainfo.replace(marker, marker + release_entry(version, date, notes), 1)


def set_release_date(metainfo: str, version: str, date: str) -> str:
    if not _DATE.match(date):
        raise ReleaseError(f"{date} is not a date such as 2026-10-20.")
    pattern = re.compile(rf'(<release version="{re.escape(version)}" date=")[^"]*(")')
    if not pattern.search(metainfo):
        raise ReleaseError(f"The metainfo has no release {version}.")
    return pattern.sub(rf"\g<1>{date}\g<2>", metainfo, count=1)


def notes_markdown(metainfo: str, version: str) -> str:
    root = ET.fromstring(metainfo)
    for release in root.iter("release"):
        if release.get("version") != version:
            continue
        lines = []
        description = release.find("description")
        for element in [] if description is None else list(description):
            if element.tag == "p":
                lines += [" ".join("".join(element.itertext()).split()), ""]
            elif element.tag in ("ul", "ol"):
                lines += [
                    f"- {' '.join(''.join(li.itertext()).split())}" for li in element.iter("li")
                ]
                lines.append("")
        return "\n".join(lines).strip() + "\n"
    raise ReleaseError(f"The metainfo has no release {version}.")


def bump(version: str, notes: list[str], date: str) -> None:
    parse_version(version)
    old = current_version()
    if parse_version(version) <= parse_version(old):
        raise ReleaseError(f"{version} is not newer than {old}.")
    cargo_toml = CARGO_TOML.read_text()
    crates = workspace_crates(cargo_toml)
    metainfo = add_release(METAINFO.read_text(), version, date, notes)
    CARGO_TOML.write_text(set_cargo_version(cargo_toml, version))
    MESON.write_text(set_meson_version(MESON.read_text(), version))
    CARGO_LOCK.write_text(set_lock_version(CARGO_LOCK.read_text(), crates, version))
    METAINFO.write_text(metainfo)
    current_version()
    # The generator leaves out path crates, so this is a guard, not the normal case.
    # A stale file makes the offline Flatpak build fail (CLAUDE.md rule 7).
    if vendors_workspace_crate(CARGO_SOURCES.read_text(), crates):
        print("cargo-sources.json lists a workspace crate. Running: just flatpak-sources")
        subprocess.run(["just", "flatpak-sources"], cwd=SOURCE_DIR, check=True)
    print(f"Version {version}. Commit, then run: git tag v{version} && git push origin v{version}")


def check(tag: str) -> None:
    version = current_version()
    if tag != f"v{version}":
        raise ReleaseError(f"The tag {tag} does not match the version {version}.")
    versions = release_versions(METAINFO.read_text())
    if not versions or versions[0] != version:
        raise ReleaseError(f"The newest metainfo release is not {version}.")
    print(f"Tag {tag} matches the version and the metainfo.")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("version")
    today = datetime.datetime.now(datetime.UTC).date().isoformat()
    bump_parser = commands.add_parser("bump")
    bump_parser.add_argument("version")
    bump_parser.add_argument("--note", action="append", default=[], required=True)
    bump_parser.add_argument("--date", default=today)
    date_parser = commands.add_parser("date")
    date_parser.add_argument("version")
    date_parser.add_argument("--date", default=today)
    notes_parser = commands.add_parser("notes")
    notes_parser.add_argument("version")
    check_parser = commands.add_parser("check")
    check_parser.add_argument("tag")
    args = parser.parse_args(argv)
    try:
        if args.command == "version":
            print(current_version())
        elif args.command == "bump":
            bump(args.version, args.note, args.date)
        elif args.command == "date":
            METAINFO.write_text(set_release_date(METAINFO.read_text(), args.version, args.date))
            print(f"Release {args.version} has the date {args.date}.")
        elif args.command == "notes":
            print(notes_markdown(METAINFO.read_text(), args.version), end="")
        else:
            check(args.tag)
    except ReleaseError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
