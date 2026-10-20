"""End-to-end test of the installed Cabin Flatpak, driven through real input.

Run it with e2e.sh, which starts the private session. The script writes a
screenshot after each step, Cabin's output, and results.json to the folder
given as its first argument. Look at the screenshots: several checks, such as
"the icons draw", need a person to confirm them.

Every desktop it makes starts with "e2e-", and it deletes them through Cabin
at the end. podman-proxy.py hides every other desktop from Cabin.
"""

import json
import os
import re
import subprocess
import sys
import time

import drive

OUT = sys.argv[1]
ONE = "e2e-one"
TWO = "e2e-two"
# Only these volumes are removed at the end. A wider match such as every
# "cabin-e2e-" volume would also take the volumes of a user's own desktop
# whose name starts with "e2e-".
VOLUMES = [f"cabin-{name}-{kind}" for name in (ONE, TWO) for kind in ("home", "state")]
shell = drive.Shell()
ui = drive.Ui(shell)
results = []
step = [0]


def shot(name):
    step[0] += 1
    path = os.path.join(OUT, f"{step[0]:02d}-{name}.png")
    shell.shot(path)
    return path


def check(name, passed, evidence):
    results.append({"check": name, "passed": bool(passed), "evidence": evidence})
    print(f"{'PASS' if passed else 'FAIL'}: {name}: {evidence}", flush=True)


def log():
    with open(os.path.join(OUT, "app.log"), errors="replace") as f:
        return f.read()


def log_since(mark):
    return log()[mark:]


def wait_log(pattern, mark, timeout):
    return drive.wait_for(
        lambda: re.search(pattern, log_since(mark)),
        timeout,
        what=f"/{pattern}/ in the log",
    )


def container_id(name):
    data = drive.inspect(name)
    return data["Id"] if data else None


def guest(name, *command, user="cabin"):
    return drive.podman("exec", "-u", user, name, *command).strip()


def guest_mode(name):
    """The current mode of the guest's monitor, such as "1176x900"."""
    out = guest(
        name,
        "sh",
        "-c",
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus gdbus call --session "
        "--dest org.gnome.Mutter.DisplayConfig --object-path /org/gnome/Mutter/DisplayConfig "
        "--method org.gnome.Mutter.DisplayConfig.GetCurrentState",
    )
    found = re.search(
        r"'(\d+x\d+)@[\d.]+', \d+, \d+, [\d.]+, [\d.]+, \[[^]]*\], \{'is-current'", out
    )
    return found.group(1) if found else None


def fingerprint(name):
    return guest(
        name,
        "openssl",
        "x509",
        "-noout",
        "-fingerprint",
        "-sha256",
        "-in",
        "/var/lib/cabin/tls/rdp.crt",
        user="root",
    )


def row_button(desktop, label):
    """The button called label in the sidebar row of desktop."""
    title = ui.find(desktop, "label")
    _, ty, _, th = ui.rect(title)
    for node in ui.find_all(label, "button"):
        _, y, _, h = ui.rect(node)
        if abs((y + h / 2) - (ty + th / 2)) < 30:
            return node
    raise LookupError(f"no {label} button beside {desktop}")


def create_desktop(name):
    ui.click_named("New Desktop", "button")
    time.sleep(1.5)
    # The alert dialogue reports its widgets away from where it draws them,
    # so its entry and buttons go through the accessibility actions.
    ui.name_new_desktop(name)
    shot(f"new-{name}")
    ui.activate(ui.find("Create", "button"))
    drive.wait_for(lambda: drive.inspect(name), 30, what=f"{name} to exist")
    time.sleep(1)


def start_and_connect(name):
    mark = len(log())
    ui.click(row_button(name, "Start"))
    wait_log(r"session connected", mark, 90)
    time.sleep(3)


def delete_desktop(name):
    ui.activate(ui.find(f"Menu for {name}", "toggle button"))
    time.sleep(1)
    ui.activate(ui.find("Delete", "button"))
    time.sleep(1)
    shot(f"delete-{name}")
    # The alert's Delete, which is the last one in the tree.
    ui.activate(ui.find_all("Delete", "button")[-1])
    drive.wait_for(lambda: drive.inspect(name) is None, 60, what=f"{name} to go")
    time.sleep(2)


def display_centre():
    window = shell.window("Cabin")
    x, y, w, h = window["frame"]
    # The desktop pane is right of the sidebar, under the header bar.
    return x + w * 0.65, y + h * 0.55


def step_start():
    app = drive.launch(os.path.join(OUT, "app.log"))
    drive.wait_for(lambda: shell.window("Cabin"), 30, what="the Cabin window")
    time.sleep(3)
    shot("start")
    text = log()
    bad = [
        line
        for line in text.splitlines()
        if re.search(r"CRITICAL|panicked|APPLICATION BUG", line)
    ]
    check(
        "1. app starts with no critical warnings",
        shell.window("Cabin") and not bad,
        f"window {shell.window('Cabin')['frame']}; bad lines: {bad[:3]}",
    )
    return app


def step_create_and_view():
    create_desktop(ONE)
    start_and_connect(ONE)
    # GNOME asks once whether Cabin may inhibit shortcuts. Allow is the
    # default button.
    asked = shell.modal_count() > 0
    shot("inhibit-question")
    if asked:
        shell.key("Return")
        time.sleep(1)
    shot("guest-desktop")
    data = drive.inspect(ONE)
    port = data["NetworkSettings"]["Ports"]["3389/tcp"][0]["HostPort"]
    check(
        "2. desktop made through New Desktop starts and the viewer connects",
        data["State"]["Running"],
        f"{ONE} {data['Id'][:12]} running on port {port}; shortcut question shown: {asked}",
    )


def step_input():
    cx, cy = display_centre()
    shell.click(cx, cy)
    time.sleep(0.5)
    shell.key("Super_L")
    time.sleep(2)
    host_overview = shell.overview_visible()
    shot("super-key")
    shell.type("term")
    time.sleep(2)
    shot("guest-search")
    shell.key("Return")
    time.sleep(4)
    # cat -v writes each key as text, so the bytes the guest received can
    # be read back from the file. Console only opens if Super opened the
    # guest's overview, so the file also shows that Super reached the guest.
    guest(ONE, "rm", "-f", KEYS_FILE)
    shell.type(f"clear; cat -v > {KEYS_FILE}\n")
    time.sleep(1)
    for key in ("F11", "F12", "Home", "Page_Up", "shift+backslash"):
        shell.key(key)
        time.sleep(0.2)
    shell.key("Return")
    time.sleep(1)
    shot("raw-keys")
    # End of input, so cat writes everything and exits.
    shell.key("ctrl+d")
    time.sleep(1)
    received = guest(ONE, "sh", "-c", f"cat {KEYS_FILE} 2>/dev/null")
    shell.type("seq 1 200\n")
    time.sleep(1)
    shell.scroll(cx, cy, "up", 5)
    time.sleep(1)
    shot("scrolled")
    zone_guest = guest(ONE, "date", "+%z")
    zone_host = subprocess.run(
        ["date", "+%z"], capture_output=True, text=True
    ).stdout.strip()
    check(
        "3. Super and the raw keys reach the guest, not the host",
        not host_overview and received == EXPECTED_KEYS,
        f"host overview after Super: {host_overview}; the guest received "
        f"{received!r}, expected {EXPECTED_KEYS!r}",
    )
    check(
        "3b. the guest uses the host time zone",
        zone_guest == zone_host,
        f"guest {zone_guest}, host {zone_host}",
    )


# Where the guest's terminal writes the keys it receives.
KEYS_FILE = "/tmp/e2e-keys"
# What cat -v writes for F11, F12, Home, Page Up and Shift+backslash.
EXPECTED_KEYS = "^[[23~^[[24~^[[H^[[5~|"


def step_resize():
    window = shell.window("Cabin")
    x, y, w, h = window["frame"]
    before = guest_mode(ONE)
    shell.drag(x + w + 2, y + h + 2, x + w - 200, y + h - 150)
    time.sleep(4)
    after = guest_mode(ONE)
    asked = re.findall(r"guest resize requested: (\d+x\d+)", log())
    shot("resized")
    check(
        "4. the guest resolution follows the window",
        after != before and asked and asked[-1] == after,
        f"guest mode {before} -> {after}; last request {asked[-1] if asked else None}",
    )


def open_settings():
    ui.click_named("Settings for this desktop", "button")
    time.sleep(1.5)


def apply_and_restart():
    shell.scroll(*display_centre(), "down", 15)
    time.sleep(1)
    ui.click(ui.find("Apply and Restart", "button"))
    time.sleep(1)
    # The confirmation is an alert dialogue, and its button is the last.
    ui.activate(ui.find_all("Apply and Restart", "button")[-1])


def step_settings():
    marker = f"e2e-marker-{int(time.time())}"
    guest(ONE, "sh", "-c", f"echo {marker} > /home/cabin/e2e-marker.txt")
    cert_before = fingerprint(ONE)
    id_before = container_id(ONE)
    open_settings()
    combo = ui.find("Renderer", "combo box")
    ui.click(combo)
    time.sleep(1)
    shot("renderer-menu")
    software = [
        n for n in ui.find_all(role="label") if n.get_name() == "Software rendering"
    ]
    ui.click(software[-1])
    time.sleep(0.5)
    mark = len(log())
    apply_and_restart()
    drive.wait_for(
        lambda: container_id(ONE) not in (None, id_before), 90, what="a new container"
    )
    wait_log(r"session connected", mark, 90)
    time.sleep(3)
    shot("after-apply")
    id_after = container_id(ONE)
    labels = drive.inspect(ONE)["Config"]["Labels"]
    config = json.loads(labels["io.github.jaydoubleu.cabin.config"])
    kept = guest(ONE, "cat", "/home/cabin/e2e-marker.txt")
    cert_after = fingerprint(ONE)
    prompt = ui.find_all("Trust the New Certificate?")
    check(
        "5. settings recreate keeps home and certificate and reconnects",
        id_after != id_before
        and kept == marker
        and cert_after == cert_before
        and not prompt
        and config["graphics"]["kind"] == "software",
        f"id {id_before[:12]} -> {id_after[:12]}; marker kept: {kept == marker}; "
        f"certificate same: {cert_after == cert_before}; trust prompt: {bool(prompt)}; "
        f"graphics {config['graphics']}",
    )


def step_failed_recreate():
    share = os.path.join(os.environ["HOME"], "e2e-share")
    os.makedirs(share, exist_ok=True)
    id_before = container_id(ONE)
    # A successful Apply closes the dialogue.
    open_settings()
    ui.click_named("Add Folder", "button")
    chooser = drive.wait_for(
        lambda: next((w for w in shell.windows() if w["title"] != "Cabin"), None),
        20,
        what="the file chooser",
    )
    time.sleep(1.5)
    shell.key("ctrl+l")
    time.sleep(0.5)
    shell.type(share + "/\n")
    time.sleep(1.5)
    shot("file-chooser")
    ui.click_in(ui.find_anywhere("Select", "button"), chooser["title"])
    time.sleep(2)
    shot("share-dialogue")
    ui.activate(ui.find("Share", "button"))
    time.sleep(1)
    # The folder goes away before Apply, so Podman refuses the new desktop.
    os.rmdir(share)
    mark = len(log())
    apply_and_restart()
    error = drive.wait_for(
        lambda: ui.find_all("Could not apply the changes"),
        60,
        what="the error dialogue",
    )
    shot("apply-refused")
    ui.activate(ui.find_all("Close", "button")[-1])
    time.sleep(1)
    ui.click(ui.find_all("Close", "button")[-1])
    reconnected = True
    try:
        wait_log(r"session connected", mark, 60)
    except TimeoutError:
        reconnected = False
    time.sleep(3)
    shot("after-refused-apply")
    data = drive.inspect(ONE)
    check(
        "6. a refused recreate keeps the old desktop, which reconnects",
        error and data["Id"] == id_before and data["State"]["Running"] and reconnected,
        f"id still {data['Id'][:12]}: {data['Id'] == id_before}; running: "
        f"{data['State']['Running']}; viewer reconnected: {reconnected}",
    )


def step_second_desktop():
    ui.click(row_button(ONE, "Stop"))
    drive.wait_for(
        lambda: not drive.inspect(ONE)["State"]["Running"], 60, what=f"{ONE} to stop"
    )
    create_desktop(TWO)

    def port(name):
        bindings = drive.inspect(name)["HostConfig"]["PortBindings"]["3389/tcp"]
        return bindings[0]["HostPort"]

    one, two = port(ONE), port(TWO)
    shot("two-desktops")
    check(
        "7. a second desktop gets its own port while the first is stopped",
        one != two,
        f"{ONE} port {one} (stopped), {TWO} port {two}",
    )


def step_delete():
    for name in (TWO, ONE):
        delete_desktop(name)
    shot("deleted")
    left = [n for n in (ONE, TWO) if drive.inspect(n)]
    volumes = [v for v in VOLUMES if v in drive.volumes("cabin-")]
    check(
        "8. delete removes the containers and both volumes",
        not left and not volumes,
        f"containers left: {left}; volumes left: {volumes}",
    )


def step_images():
    ui.activate(ui.find("Main menu", "toggle button"))
    time.sleep(1)
    ui.activate(ui.find("Guest Images", "button"))
    time.sleep(3)
    shot("guest-images")
    group = ui.find("Built images")
    # Each row names its image in the title or the subtitle, and the
    # subtitle always carries the short id.
    rows = [n for n in ui.nodes(group) if n.get_role_name() == "list item"]
    text = " ".join(
        n.get_name() for n in ui.nodes(group) if n.get_role_name() == "label"
    )
    listed = set(re.findall(r"\b[0-9a-f]{12}\b", text))
    out = drive.podman(
        "images",
        "--filter",
        "label=io.github.jaydoubleu.cabin.guest=true",
        "--format",
        "{{.ID}}",
    )
    ours = {i[:12] for i in out.split()}
    everything = {i[:12] for i in drive.podman("images", "--format", "{{.ID}}").split()}
    check(
        "9. Guest Images lists only Cabin images",
        listed == ours,
        f"{len(rows)} rows listing {sorted(listed)}; images with the Cabin label "
        f"{sorted(ours)}; images on the host: {len(everything)}",
    )
    shell.key("Escape")


def cleanup():
    """Removes any test desktop that a failed step left behind."""
    for name in (ONE, TWO):
        if drive.inspect(name):
            drive.podman("rm", "--force", name, check=False)
    present = drive.volumes("cabin-")
    for volume in VOLUMES:
        if volume in present:
            drive.podman("volume", "rm", volume, check=False)


def main():
    os.makedirs(OUT, exist_ok=True)
    taken = [name for name in (ONE, TWO) if drive.inspect(name)]
    present = drive.volumes("cabin-")
    taken += [v for v in VOLUMES if v in present]
    if taken:
        sys.exit(f"These exist already and may be yours: {taken}. Remove them first.")
    before = drive.managed_containers()
    shell.call("HideOverview")
    app = step_start()
    steps = [
        step_create_and_view,
        step_input,
        step_resize,
        step_settings,
        step_failed_recreate,
        step_second_desktop,
        step_delete,
        step_images,
    ]
    try:
        for run in steps:
            try:
                run()
            except Exception as e:  # noqa: BLE001 - one step failing must not hide the rest
                shot(f"error-{run.__name__}")
                check(run.__name__, False, f"{type(e).__name__}: {e}")
                # Close whatever dialogue the step left open, so the next
                # step starts from the main window.
                for _ in range(3):
                    shell.key("Escape")
                    time.sleep(0.5)
    finally:
        text = log()
        bad = [
            line
            for line in text.splitlines()
            if re.search(r"CRITICAL|panicked|APPLICATION BUG", line)
        ]
        check(
            "output has no critical lines",
            not bad,
            f"{len(bad)} lines, first: {bad[:2]}",
        )
        app.terminate()
        cleanup()
        left = drive.strays(before, (ONE, TWO))
        check(
            "no desktop is left under a name the test did not choose",
            not left,
            drive.stray_message(left) if left else "none",
        )
        with open(os.path.join(OUT, "results.json"), "w") as f:
            json.dump(results, f, indent=2)
    sys.exit(0 if all(r["passed"] for r in results) else 1)


main()
