"""Takes the screenshots for the README and the metainfo.

Run it with take.sh, which starts the private session. The first argument is
the folder for the finished images, and the second is a work folder for
Cabin's output and the shots that are not kept.

The script makes three demo desktops with natural names, because the
screenshots show them. podman-proxy.py hides every other desktop, and the
script stops at once if a desktop with one of these names exists already, so
it never touches a desktop that belongs to the user. At the end it deletes
the demo desktops and their volumes.

There is no shot of Connect From Another Device. That dialogue shows the
real addresses of this computer, and turning remote access on would publish
a demo desktop on the real network while the script runs.
"""

import os
import re
import sys
import time

import drive

OUT = WORK = None
RUNNING = "fedora-dev"
STOPPED = ["web-project", "sandbox"]
DEMO = [RUNNING, *STOPPED]
# The finished window, without the shadow. 16:10, as most laptops are.
WIDTH, HEIGHT = 1440, 900
shell = drive.Shell()
ui = drive.Ui(shell)


def log():
    with open(os.path.join(WORK, "app.log"), errors="replace") as f:
        return f.read()


def work(name):
    shell.shot(os.path.join(WORK, f"{name}.png"))


def keep(name):
    """Writes the Cabin window, with its shadow, as a finished image."""
    path = os.path.join(OUT, f"{name}.png")
    shell.shot_window(path)
    print(f"wrote {path}", flush=True)


def row_button(desktop, label):
    """The button called label in the sidebar row of desktop."""
    title = ui.find(desktop, "label")
    _, ty, _, th = ui.rect(title)
    for node in ui.find_all(label, "button"):
        _, y, _, h = ui.rect(node)
        if abs((y + h / 2) - (ty + th / 2)) < 30:
            return node
    raise LookupError(f"no {label} button beside {desktop}")


def size_window():
    """Gives the window the finished size, near the top left corner.

    The shell cannot resize a client side decorated window for us, so the
    pointer drags the header bar and then the corner, as a user does.
    """
    window = shell.window("Cabin")
    if window["maximized"]:
        x, y, w, _ = window["frame"]
        shell.double_click(x + w / 2, y + 22)
        time.sleep(1.5)
        window = shell.window("Cabin")
    x, y, w, h = window["frame"]
    # The middle of the header bar above the viewer is empty.
    grab_x, grab_y = x + w * 0.62, y + 22
    shell.drag(grab_x, grab_y, grab_x - x + 60, grab_y - y + 60)
    time.sleep(1)
    for _ in range(3):
        x, y, w, h = shell.window("Cabin")["frame"]
        if (w, h) == (WIDTH, HEIGHT):
            break
        shell.drag(x + w + 2, y + h + 2, x + WIDTH + 2, y + HEIGHT + 2, steps=20)
        time.sleep(1)
    print(f"window frame {shell.window('Cabin')['frame']}", flush=True)


def create_desktop(name):
    ui.click_named("New Desktop", "button")
    time.sleep(1.5)
    # The alert dialogue reports its widgets away from where it draws them,
    # so its entry and buttons go through the accessibility actions.
    ui.set_text(ui.find("Name", "text"), name)
    time.sleep(1)
    ui.activate(ui.find("Create", "button"))
    drive.wait_for(lambda: drive.inspect(name), 30, what=f"{name} to exist")
    time.sleep(1.5)


def start_and_open(name):
    mark = len(log())
    ui.click(row_button(name, "Start"))
    # Starting a desktop does not open it. Selecting the row does.
    time.sleep(1)
    ui.click_named(name, "label")
    drive.wait_for(
        lambda: re.search(r"session connected", log()[mark:]),
        120,
        what="the viewer to connect",
    )
    time.sleep(3)
    # GNOME asks once whether Cabin may inhibit shortcuts. Allow is the
    # default button.
    if shell.modal_count() > 0:
        shell.key("Return")
        time.sleep(1)


def guest_scene():
    """Opens Console and Files in the guest, then the guest's overview."""
    x, y, w, h = shell.window("Cabin")["frame"]
    shell.click(x + w * 0.65, y + h * 0.55)
    # The session needs some seconds after the first frame before its
    # search finds applications.
    time.sleep(8)
    # "terminal" finds characters first. The application is called Console.
    launch_in_guest("term")
    shell.type("cat /etc/fedora-release\n")
    time.sleep(0.5)
    shell.type("podman --version; flatpak --version\n")
    time.sleep(1.5)
    work("guest-console")
    launch_in_guest("files")
    work("guest-files")
    shell.key("Super_L")
    time.sleep(3)
    # The guest draws a hover effect and a tooltip under the pointer. Park
    # it on the empty background, below the sidebar's end of the viewer.
    shell.move(x + w * 0.3, y + h * 0.9)
    time.sleep(1.5)


def launch_in_guest(search):
    """Starts an application in the guest from the search of its overview."""
    shell.key("Super_L")
    time.sleep(2)
    shell.type(search)
    time.sleep(3)
    shell.key("Return")
    time.sleep(5)


def settings_scene(name):
    """Opens the settings of name with one shared folder added.

    The share is not applied. Escape closes the dialogue and drops it.
    """
    share = os.path.join(os.environ["HOME"], "Projects", name)
    os.makedirs(share, exist_ok=True)
    ui.click_named(name, "label")
    time.sleep(1.5)
    ui.click_named("Settings for this desktop", "button")
    time.sleep(2)
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
    ui.click_in(ui.find_anywhere("Select", "button"), chooser["title"])
    time.sleep(2)
    work("share-dialogue")
    ui.activate(ui.find("Share", "button"))
    time.sleep(1.5)


def images_scene():
    ui.activate(ui.find("Main menu", "toggle button"))
    time.sleep(1)
    ui.activate(ui.find("Guest Images", "button"))
    time.sleep(3)
    # The Escape that closed the last dialogue makes GTK draw a focus ring
    # in this one. A click on the empty foot of the dialogue hides it.
    x, y, w, h = shell.window("Cabin")["frame"]
    shell.click(x + w * 0.5, y + h * 0.78)
    time.sleep(1)


def escape(times=1):
    for _ in range(times):
        shell.key("Escape")
        time.sleep(1)


def cleanup():
    for name in DEMO:
        if drive.inspect(name):
            drive.podman("rm", "--force", name, check=False)
        for volume in (f"cabin-{name}-home", f"cabin-{name}-state"):
            drive.podman("volume", "rm", volume, check=False)


def main():
    global OUT, WORK
    OUT, WORK = sys.argv[1], sys.argv[2]
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(WORK, exist_ok=True)
    taken = [name for name in DEMO if drive.inspect(name)]
    volumes = [v for name in DEMO for v in drive.volumes(f"cabin-{name}-")]
    if taken or volumes:
        sys.exit(
            f"These exist already and may be yours: {taken + volumes}. "
            "Remove them or choose other names."
        )
    shell.call("HideOverview")
    names = [f"^{re.escape(name)}$" for name in DEMO]
    app = drive.launch(os.path.join(WORK, "app.log"), names, hide_dangling=True)
    try:
        drive.wait_for(lambda: shell.window("Cabin"), 30, what="the Cabin window")
        time.sleep(3)
        size_window()
        for name in DEMO:
            create_desktop(name)
        start_and_open(RUNNING)
        guest_scene()
        keep("main")

        # A name is typed, and Cancel leaves the list as it was.
        ui.click_named("New Desktop", "button")
        time.sleep(1.5)
        ui.set_text(ui.find("Name", "text"), "testing")
        time.sleep(1)
        keep("new-desktop")
        ui.activate(ui.find("Cancel", "button"))
        time.sleep(1)

        settings_scene(STOPPED[0])
        keep("settings")
        escape()

        images_scene()
        keep("images")
        escape()
        work("end")
    finally:
        app.terminate()
        time.sleep(2)
        cleanup()


if __name__ == "__main__":
    main()
