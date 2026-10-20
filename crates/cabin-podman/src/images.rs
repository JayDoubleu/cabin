//! Guest image operations: list, build and remove.
//!
//! Cabin's guest image is built on the machine that runs it, from the
//! `guest/` directory shipped with the application. It is never published,
//! so a fresh installation has no image and the first desktop cannot start.
//! See DESIGN.md section 19.

use crate::client::extract_message;
use crate::error::{Error, Result};
use crate::http::{self, RawBody};
use crate::Podman;
use serde::Deserialize;

/// A byte count a person can read.
///
/// Powers of 1000, not 1024, because that is what Podman prints and a manager
/// that disagreed with `podman images` about a size would be worse than one
/// that rounds differently from the disk.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "kB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000.0 && unit + 1 < UNITS.len() {
        size /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size:.0} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// The label that marks an image as Cabin's. Set in `guest/Containerfile`, so
/// an image built with `just guest` carries it too.
pub const LABEL_GUEST: &str = "io.github.jaydoubleu.cabin.guest";

/// The image a desktop is built from, as Podman reports it.
#[derive(Debug, Clone)]
pub struct Image {
    pub id: String,
    /// Every name this image answers to, for example
    /// `localhost/cabin-guest:f44`.
    pub tags: Vec<String>,
    pub size_bytes: u64,
    /// Seconds since the epoch.
    pub created: i64,
    /// True when the image carries Cabin's guest label.
    pub is_guest: bool,
    /// True when nothing references the image by name any more. A rebuild
    /// leaves the image it replaced in this state. A dangling image is not
    /// necessarily unused: every desktop made from the old build still runs
    /// on it. See [`Image::prunable`].
    pub dangling: bool,
    /// True when at least one container, running or not, uses the image.
    pub in_use: bool,
}

impl Image {
    /// True when Podman's default prune would remove this image.
    ///
    /// Podman prunes only dangling images that no container uses. Counting
    /// the others would promise space the prune does not free, and would
    /// list images that existing desktops still run on.
    pub fn prunable(&self) -> bool {
        self.dangling && !self.in_use
    }

    /// The name to show. An image can carry several, and it can carry none
    /// when a rebuild has left the old one untagged.
    ///
    /// No angle brackets. This ends up in interface labels that parse markup,
    /// where `<untagged>` reads as an unclosed tag and the label is dropped
    /// with a warning rather than shown.
    pub fn display_name(&self) -> String {
        self.tags
            .first()
            .cloned()
            .unwrap_or_else(|| format!("Untagged {}", self.short_id()))
    }

    pub fn short_id(&self) -> String {
        self.id
            .trim_start_matches("sha256:")
            .chars()
            .take(12)
            .collect()
    }

    /// A size a person can read, in the units Podman itself uses.
    pub fn human_size(&self) -> String {
        human_bytes(self.size_bytes)
    }
}

#[derive(Deserialize)]
struct ImageSummary {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Names", default)]
    names: Option<Vec<String>>,
    #[serde(rename = "RepoTags", default)]
    repo_tags: Option<Vec<String>>,
    #[serde(rename = "Size", default)]
    size: u64,
    #[serde(rename = "Created", default)]
    created: i64,
    #[serde(rename = "Labels", default)]
    labels: Option<std::collections::HashMap<String, String>>,
    /// How many containers use the image.
    #[serde(rename = "Containers", default)]
    containers: i64,
}

/// One line of build output.
#[derive(Deserialize)]
struct BuildLine {
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl Podman {
    /// True when an image with this name is present.
    pub async fn image_exists(&self, name: &str) -> Result<bool> {
        let path = format!("/v5.0.0/libpod/images/{}/exists", encode(name));
        let response = http::request(self.socket_path(), "GET", &path, None).await?;
        match response.status {
            204 => Ok(true),
            404 => Ok(false),
            status => Err(Error::Api {
                status,
                message: extract_message(&response.body),
            }),
        }
    }

    /// Every image on the host, newest first.
    pub async fn list_images(&self) -> Result<Vec<Image>> {
        let response = http::request(
            self.socket_path(),
            "GET",
            "/v5.0.0/libpod/images/json",
            None,
        )
        .await?;
        if response.status >= 400 {
            return Err(Error::Api {
                status: response.status,
                message: extract_message(&response.body),
            });
        }
        let summaries: Vec<ImageSummary> = serde_json::from_slice(&response.body)?;
        let mut images: Vec<Image> = summaries
            .into_iter()
            .map(|s| {
                // Podman fills one or the other depending on the endpoint
                // version, so take whichever is there.
                let tags = s.names.or(s.repo_tags).unwrap_or_default();
                let is_guest = s
                    .labels
                    .as_ref()
                    .and_then(|l| l.get(LABEL_GUEST))
                    .is_some_and(|v| v == "true");
                Image {
                    dangling: tags.is_empty(),
                    in_use: s.containers > 0,
                    // An image built before the label existed still carries
                    // the name, so fall back to it rather than hiding an
                    // image the user plainly recognises.
                    is_guest: is_guest || tags.iter().any(|t| t.contains("cabin-guest")),
                    id: s.id,
                    tags,
                    size_bytes: s.size,
                    created: s.created,
                }
            })
            .collect();
        images.sort_by_key(|i| std::cmp::Reverse(i.created));
        Ok(images)
    }

    /// Builds `tag` from a build context, reporting output as it arrives.
    ///
    /// The context is a tar archive, which is what the build endpoint takes.
    /// Output has to be streamed: a guest image takes minutes to build, and a
    /// progress display that only appears at the end is not a progress
    /// display.
    pub async fn build_image<F>(&self, tag: &str, context: Vec<u8>, mut on_output: F) -> Result<()>
    where
        F: FnMut(&str),
    {
        let path = format!(
            "/v5.0.0/libpod/build?dockerfile=Containerfile&t={}&pull=true",
            encode(tag)
        );
        let mut failure: Option<String> = None;
        http::stream_request(
            self.socket_path(),
            "POST",
            &path,
            Some(RawBody {
                content_type: "application/x-tar",
                bytes: &context,
            }),
            |line| {
                let Ok(parsed) = serde_json::from_str::<BuildLine>(line) else {
                    return;
                };
                if let Some(text) = parsed.stream {
                    on_output(&text);
                }
                // The stream reports a failed build in its body and still
                // ends with a 200, so the error has to be read out of it.
                if let Some(text) = parsed.error {
                    on_output(&text);
                    failure = Some(text);
                }
            },
        )
        .await?;

        if let Some(message) = failure {
            return Err(Error::Api {
                status: 200,
                message,
            });
        }
        // A stream can end cleanly with nothing tagged. Saying the image is
        // ready would then send the user to a desktop that cannot be made.
        if !self.image_exists(tag).await? {
            return Err(Error::Protocol(format!(
                "the build ended, but Podman has no image called {tag}"
            )));
        }
        Ok(())
    }

    /// Removes an image. Podman refuses while a container still uses it.
    pub async fn remove_image(&self, name: &str, force: bool) -> Result<()> {
        let path = format!("/v5.0.0/libpod/images/{}?force={force}", encode(name));
        let response = http::request(self.socket_path(), "DELETE", &path, None).await?;
        if response.status >= 400 {
            return Err(Error::Api {
                status: response.status,
                message: extract_message(&response.body),
            });
        }
        Ok(())
    }
}

impl Podman {
    /// Removes every image that nothing names and no container uses.
    ///
    /// Returns the number removed and the bytes reclaimed. This is not
    /// limited to Cabin's images: Podman prunes by danglingness, not by
    /// owner, and the interface says so before it is used.
    pub async fn prune_images(&self) -> Result<(usize, u64)> {
        let response = http::request(
            self.socket_path(),
            "POST",
            "/v5.0.0/libpod/images/prune",
            Some(b"{}"),
        )
        .await?;
        if response.status >= 400 {
            return Err(Error::Api {
                status: response.status,
                message: extract_message(&response.body),
            });
        }
        // A prune that removed nothing answers with null rather than an
        // empty list, which is not the same shape.
        let reports: Vec<PruneReport> = serde_json::from_slice(&response.body).unwrap_or_default();
        let freed = reports.iter().map(|r| r.size).sum();
        Ok((reports.len(), freed))
    }
}

#[derive(Deserialize, Default)]
struct PruneReport {
    #[serde(rename = "Size", default)]
    size: u64,
}

/// True for a file that belongs to Cabin's own build rather than to the guest.
///
/// A source checkout carries `meson.build` beside the Containerfile, because
/// that is how the guest files get installed. It is not part of the guest, and
/// it should reach neither the build context nor a copy the user takes: seeing
/// it in their own recipe folder invites the question of what it does there.
pub fn is_build_system_file(name: &str) -> bool {
    name == "meson.build"
}

/// Packs a directory into the tar archive that the build endpoint takes.
///
/// Only the execute bit is carried. Every file is 0755 or 0644, so the archive
/// is the same for a given tree however the files were checked out, and
/// Podman's layer cache still works. The execute bit itself has to survive,
/// because a custom recipe can `COPY` a script and run it without a `chmod`.
///
/// A symbolic link to a file is packed as that file. Any other link, or a
/// special file, is an error that names the path.
pub fn build_context(dir: &std::path::Path) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    append_dir(&mut builder, dir, std::path::Path::new(""))?;
    builder
        .into_inner()
        .map_err(|e| Error::Protocol(format!("could not pack the build context: {e}")))
}

/// The mode a file gets in the build context: executable or not, nothing else.
fn archive_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o100 != 0 {
        0o755
    } else {
        0o644
    }
}

fn append_dir(
    builder: &mut tar::Builder<Vec<u8>>,
    dir: &std::path::Path,
    prefix: &std::path::Path,
) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::Protocol(format!("could not read {}: {e}", dir.display())))?
        .filter_map(|e| e.ok())
        .collect();
    // Sorted, so the same tree always produces the same archive and Podman's
    // layer cache is not defeated by the order the filesystem happens to
    // return.
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = prefix.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|e| Error::Protocol(format!("could not read {}: {e}", path.display())))?;

        if file_type.is_dir() {
            append_dir(builder, &path, &name)?;
            continue;
        }
        // A link to a file is packed as the file it points at, because a
        // COPY in the recipe means the content. Anything else is refused by
        // name. Leaving it out quietly made a COPY fail later with a message
        // that did not point at the link.
        let metadata = std::fs::metadata(&path)
            .map_err(|e| Error::Protocol(format!("could not read {}: {e}", path.display())))?;
        if !metadata.is_file() {
            return Err(Error::Protocol(format!(
                "{} is not a regular file, so it cannot go into the build. \
                 Replace it with a copy of what it points at.",
                path.display()
            )));
        }
        if is_build_system_file(&entry.file_name().to_string_lossy()) {
            continue;
        }

        let contents = std::fs::read(&path)
            .map_err(|e| Error::Protocol(format!("could not read {}: {e}", path.display())))?;
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(archive_mode(&metadata));
        header.set_mtime(0);
        header.set_cksum();
        builder
            .append_data(&mut header, &name, contents.as_slice())
            .map_err(|e| Error::Protocol(format!("could not pack {}: {e}", path.display())))?;
    }
    Ok(())
}

/// Percent encodes the characters that appear in an image name.
///
/// A tag holds a colon and a slash, and both end a path segment if they are
/// left as they are.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_separators_in_an_image_name() {
        // Left alone, the slash starts a new path segment and the request
        // reaches the wrong endpoint.
        assert_eq!(
            encode("localhost/cabin-guest:f44"),
            "localhost%2Fcabin-guest%3Af44"
        );
    }

    fn image(size: u64, tags: &[&str]) -> Image {
        Image {
            id: "sha256:0123456789abcdef".into(),
            dangling: tags.is_empty(),
            in_use: false,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            size_bytes: size,
            created: 0,
            is_guest: false,
        }
    }

    #[test]
    fn an_untagged_image_a_desktop_uses_is_not_prunable() {
        // A rebuild of the base leaves the old image untagged while every
        // existing desktop still runs on it. Podman will not prune it.
        let mut old = image(1, &[]);
        assert!(old.prunable());
        old.in_use = true;
        assert!(old.dangling);
        assert!(!old.prunable());
        assert!(!image(1, &["localhost/x:1"]).prunable());
    }

    fn archive_modes(archive: &[u8]) -> Vec<(String, u32)> {
        let mut found: Vec<(String, u32)> = tar::Archive::new(archive)
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.path().unwrap().display().to_string(),
                    e.header().mode().unwrap(),
                )
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn the_execute_bit_survives_and_nothing_else_does() {
        // A custom recipe may COPY a script and run it without a chmod.
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cabin-tar-modes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.sh"), b"#!/bin/sh\n").unwrap();
        std::fs::write(dir.join("notes"), b"x").unwrap();
        std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::set_permissions(dir.join("notes"), std::fs::Permissions::from_mode(0o600))
            .unwrap();

        let archive = build_context(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            archive_modes(&archive),
            vec![("notes".to_string(), 0o644), ("run.sh".to_string(), 0o755)]
        );
    }

    #[test]
    fn a_link_to_a_file_is_packed_as_the_file() {
        let dir = std::env::temp_dir().join(format!("cabin-tar-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("real"), b"content").unwrap();
        std::os::unix::fs::symlink("real", dir.join("alias")).unwrap();

        let archive = build_context(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let mut entries = tar::Archive::new(archive.as_slice());
        let mut contents = std::collections::BTreeMap::new();
        for entry in entries.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().display().to_string();
            let mut text = String::new();
            std::io::Read::read_to_string(&mut entry, &mut text).unwrap();
            contents.insert(path, text);
        }
        assert_eq!(contents.get("alias").map(String::as_str), Some("content"));
    }

    #[test]
    fn a_broken_link_is_named_rather_than_skipped() {
        let dir = std::env::temp_dir().join(format!("cabin-tar-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink("missing", dir.join("dangling")).unwrap();

        let result = build_context(&dir);
        std::fs::remove_dir_all(&dir).ok();

        let message = result.expect_err("a broken link must fail").to_string();
        assert!(message.contains("dangling"), "{message}");
    }

    #[test]
    fn an_image_with_no_name_is_dangling() {
        // A rebuild leaves the image it replaced in this state, and those
        // are what fill a disk without anything accounting for them.
        assert!(image(1, &[]).dangling);
        assert!(!image(1, &["localhost/cabin-guest:f44"]).dangling);
    }

    #[test]
    fn reports_size_the_way_podman_does() {
        assert_eq!(image(512, &[]).human_size(), "512 B");
        assert_eq!(image(190_000_000, &[]).human_size(), "190.0 MB");
        assert_eq!(image(1_700_000_000, &[]).human_size(), "1.7 GB");
    }

    #[test]
    fn packs_a_directory_tree_into_a_readable_archive() {
        let dir = std::env::temp_dir().join(format!("cabin-tar-{}", std::process::id()));
        let nested = dir.join("scripts");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.join("Containerfile"), b"FROM scratch\n").unwrap();
        std::fs::write(nested.join("prepare.sh"), b"#!/bin/sh\n").unwrap();

        let archive = build_context(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let mut found: Vec<String> = tar::Archive::new(archive.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        found.sort();
        // The paths must be relative to the context root. An absolute or
        // dot-prefixed path makes the build fail to find the Containerfile.
        assert_eq!(found, vec!["Containerfile", "scripts/prepare.sh"]);
    }

    #[test]
    fn the_build_system_file_is_left_out() {
        // A source checkout carries meson.build beside the Containerfile. It
        // has nothing to do with the guest and should not be sent to Podman.
        let dir = std::env::temp_dir().join(format!("cabin-tar-meson-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Containerfile"), b"FROM scratch\n").unwrap();
        std::fs::write(dir.join("meson.build"), b"install_data()\n").unwrap();

        let archive = build_context(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let found: Vec<String> = tar::Archive::new(archive.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert_eq!(found, vec!["Containerfile"]);
    }

    #[test]
    fn packing_the_same_tree_twice_gives_the_same_archive() {
        // Otherwise every build would miss Podman's layer cache.
        let dir = std::env::temp_dir().join(format!("cabin-tar-stable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b"), b"b").unwrap();
        std::fs::write(dir.join("a"), b"a").unwrap();
        let first = build_context(&dir).unwrap();
        let second = build_context(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(first, second);
    }

    #[test]
    fn names_an_untagged_image_by_its_id() {
        // A rebuild leaves the previous image tagless, and it still has to be
        // possible to point at it and remove it.
        let untagged = image(1, &[]);
        assert!(untagged.display_name().contains("012345678"));
        assert_eq!(image(1, &["localhost/x:1"]).display_name(), "localhost/x:1");
    }

    #[test]
    fn a_display_name_carries_no_markup() {
        // These go into labels that parse markup. An angle bracket there is
        // read as a tag, and the label is dropped with a warning.
        for image in [image(1, &[]), image(1, &["localhost/x:1"])] {
            let name = image.display_name();
            assert!(!name.contains('<'), "{name}");
            assert!(!name.contains('>'), "{name}");
            assert!(!name.contains('&'), "{name}");
        }
    }
}
