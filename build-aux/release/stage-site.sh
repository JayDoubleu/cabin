#!/usr/bin/env bash
# Writes the GitHub Pages site: the repository, the client files and a landing page.
#
# Usage: stage-site.sh REPO_DIR SITE_DIR
#
# SITE_DIR/repo/                    the OSTree repository
# SITE_DIR/$APP_ID.flatpakref       installs the app and adds the remote
# SITE_DIR/$REMOTE_NAME.flatpakrepo adds the remote only
# SITE_DIR/signing-key.asc          the public key
# SITE_DIR/index.html               the install commands for people
set -euo pipefail
# shellcheck source=build-aux/release/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

REPO="${1:?usage: stage-site.sh REPO_DIR SITE_DIR}"
SITE="${2:?usage: stage-site.sh REPO_DIR SITE_DIR}"
[[ -f "$REPO/config" ]] || die "$REPO is not an OSTree repository"

rm -rf "$SITE"
mkdir -p "$SITE"
cp -a "$REPO" "$SITE/repo"
cp "$PUBLIC_KEY" "$SITE/signing-key.asc"
# The source file has a short name. meson installs it as $APP_ID.svg.
cp "$SOURCE_DIR/data/icons/hicolor/scalable/apps/cabin.svg" "$SITE/icon.svg"

GPG_KEY="$(key_base64)"
cat > "$SITE/$APP_ID.flatpakref" <<EOF
[Flatpak Ref]
Name=$APP_ID
Branch=$BRANCH
Title=$APP_TITLE
Url=$PAGES_URL/repo/
SuggestRemoteName=$REMOTE_NAME
RuntimeRepo=$RUNTIME_REPO
Homepage=$HOMEPAGE
Icon=$PAGES_URL/icon.svg
IsRuntime=false
GPGKey=$GPG_KEY
EOF

cat > "$SITE/$REMOTE_NAME.flatpakrepo" <<EOF
[Flatpak Repo]
Title=$APP_TITLE
Comment=Run Fedora GNOME desktops in containers
Url=$PAGES_URL/repo/
Homepage=$HOMEPAGE
Icon=$PAGES_URL/icon.svg
DefaultBranch=$BRANCH
GPGKey=$GPG_KEY
EOF

VERSION="$(app_version)"
cat > "$SITE/index.html" <<EOF
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Cabin Flatpak repository</title>
<link rel="icon" href="icon.svg">
<style>
  :root { color-scheme: light dark; --bg: #fafafb; --fg: #1e1e22; --muted: #5e5c64; --code: #ebebed; }
  @media (prefers-color-scheme: dark) { :root { --bg: #222226; --fg: #f6f5f4; --muted: #b0afb5; --code: #36363a; } }
  body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.5 system-ui, sans-serif; }
  main { max-width: 46rem; margin: 0 auto; padding: 3rem 1rem; }
  header { display: flex; gap: 1rem; align-items: center; }
  header img { width: 64px; height: 64px; }
  h1 { margin: 0; }
  p.muted { color: var(--muted); margin: 0; }
  pre { background: var(--code); padding: 0.75rem 1rem; border-radius: 8px; overflow-x: auto; }
  a { color: #3584e4; }
</style>
</head>
<body>
<main>
<header>
  <img src="icon.svg" alt="">
  <div><h1>Cabin</h1><p class="muted">Run Fedora GNOME desktops in containers. Version $VERSION.</p></div>
</header>
<h2>Install</h2>
<p>This command adds the Cabin repository and installs the app. The GNOME runtime comes from Flathub.</p>
<pre>flatpak install --user --from $PAGES_URL/$APP_ID.flatpakref</pre>
<p>GNOME Software and <code>flatpak update</code> then get new versions.</p>
<h2>Before the first start</h2>
<p>Cabin runs each desktop in rootless Podman 5 on the host. Turn on the Podman socket for your user:</p>
<pre>systemctl --user enable --now podman.socket</pre>
<p>On first use, open Guest Images from the main menu and build the guest image. Cabin builds it on your computer and never pulls it from a registry. The first build takes several minutes.</p>
<p>A container shares the host kernel. Cabin is not a sandbox for code that you do not trust. Use a virtual machine for that.</p>
<h2>Add the repository only</h2>
<pre>flatpak remote-add --user --if-not-exists $REMOTE_NAME $PAGES_URL/$REMOTE_NAME.flatpakrepo
flatpak install --user $REMOTE_NAME $APP_ID</pre>
<p>The repository is signed with the key in <a href="signing-key.asc">signing-key.asc</a> ($(key_id)).</p>
<p><a href="$HOMEPAGE">Source code, documentation and issues</a></p>
</main>
</body>
</html>
EOF

echo "Site in $SITE:"
ls -la "$SITE"
du -sh "$SITE/repo"
