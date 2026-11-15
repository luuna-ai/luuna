# Release manifest format

No release of Luuna has been published yet, so this folder holds `INDEX.tsv` with its header line only and no release folder. This page documents what `packaging/installer/install-manifest.sh` validates and what `packaging/installer/tests/check-manifests.sh` checks. Both are the authority if this page and they differ.

## Manifest

A manifest is a tab-separated text file: LF line endings only (no CR), printable ASCII only, and a final newline.

```
luuna-release-manifest<TAB>1
release<TAB><version>
asset<TAB><component><TAB><version><TAB><os><TAB><arch><TAB><url><TAB><sha256><TAB><file><TAB><exe,exe,...>
```

Line 1 is the header and line 2 the release line. At least one `asset` row follows. Each row has exactly nine fields, none empty.

- `version`: `N.N.N` with an optional `-prerelease` suffix (letters, digits, `.` and `-`, starting with a letter or digit), at most 64 characters. Each row's version equals the release line.
- `component`: `luuna`, the only entry of the component table. Link names such as `luuna` and `lu` are never manifest entries.
- `os`: `darwin` or `linux`. `arch`: `amd64` or `arm64`. There is one row per component, os and arch.
- `sha256`: 64 lowercase hexadecimal characters, the SHA-256 of the archive.
- `file`: exactly `<component>_<version>_<os>_<arch>.tar.gz`.
- `url`: exactly `https://github.com/luuna-ai/luuna/releases/download/v<version>/<file>`. One repository and one tag form (`v<version>`) are allowed; any other URL is refused.
- `exes`: the executables of the component, comma separated, each exactly once and nothing else, in any order. For `luuna`: `luuna-tui`, `luuna-runtime`, `luuna-sbx`.

Selecting a platform needs a row for every component in the table for the host os and arch; otherwise the installer exits 3. There are no Windows assets.

## Archive layout

The archive is a gzip-compressed tar. Its listing may contain only `bin/` and `bin/<exe>` for the row's executables. Every executable must be present. Entries are regular files and directories only: no symbolic link, hard link, device or fifo, no duplicate, no absolute or `..` path. The installer checks the listing before extraction and re-checks the extracted count and that each executable is a regular executable file. No Bun, component tree or other payload is allowed in this format version.

## Index

`INDEX.tsv` pins released manifests. Line 1 is `#release-version<TAB>manifest-sha256`. Each later line is `<version><TAB><sha256 of releases/<version>/manifest.tsv>`, with a valid, unique version and a 64-character lowercase hexadecimal checksum. `install.sh --version V` reads `releases/V/manifest.tsv` and requires its SHA-256 to equal the index line for `V`.

Each indexed version has a folder `releases/<version>/` holding only `manifest.tsv`, which must be valid, whose release line equals the folder name, and whose SHA-256 equals the index line. A folder that is not indexed, a symbolic link, or any other file or entry in this folder (besides `FORMAT.md` and `INDEX.tsv`) is an error. The index file has LF endings and a final newline.

## Append-only rule

`sh packaging/installer/tests/check-manifests.sh [--base REF] [--root DIR]` validates the above for the repository at `DIR` (default: this checkout). With `--base REF` it also compares with `REF`: released files may only be added, never changed or removed; `INDEX.tsv` may only gain lines appended after the existing ones; `FORMAT.md` may change; any other change under `packaging/installer/releases/` is refused.

No manifest or checksum is committed here before a release exists. Building, signing and publishing a release, and adding its manifest and index line, are owner gates.
