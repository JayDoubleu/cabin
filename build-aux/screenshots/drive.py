"""Drives Cabin in the private session that session.sh starts.

Run a script that imports this with "session.sh run python3 SCRIPT". The
scripts run on the host, outside the Flatpak, and send real input through the
cabin-shot@local extension. They find widgets through the accessibility bus,
which also checks that the widgets have names a screen reader can use.
"""

import json
import os
import subprocess
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gio, GLib  # noqa: E402

NAME = "io.github.jaydoubleu.CabinShot"
PATH = "/io/github/jaydoubleu/CabinShot"
APP_ID = "io.github.jaydoubleu.Cabin"


class Shell:
    """The shot helper extension in the private GNOME Shell."""

    def __init__(self):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION)

    def call(self, method, signature=None, args=(), reply="()", timeout=30000):
        result = self.bus.call_sync(
            NAME,
            PATH,
            NAME,
            method,
            GLib.Variant(signature, tuple(args)) if signature else None,
            GLib.VariantType(reply),
            Gio.DBusCallFlags.NONE,
            timeout,
            None,
        )
        return result.unpack()

    def shot(self, path):
        self.call("Shot", "(s)", [path])

    def shot_window(self, path):
        self.call("ShotWindow", "(s)", [path])

    def overview_visible(self):
        return self.call("OverviewVisible", reply="(b)")[0]

    def modal_count(self):
        return self.call("ModalCount", reply="(i)")[0]

    def windows(self):
        return json.loads(self.call("ListWindows", reply="(s)")[0])

    def window(self, title="Cabin"):
        for window in self.windows():
            if title in (window["title"] or ""):
                return window
        return None

    def activate(self, title):
        return self.call("Activate", "(s)", [title], "(b)")[0]

    def maximize(self, title):
        return self.call("Maximize", "(s)", [title], "(b)")[0]

    def double_click(self, x, y):
        self.click(x, y)
        time.sleep(0.1)
        self.click(x, y)

    def button(self, pressed, button=1):
        self.call("Button", "(ub)", [button, pressed])

    def drag(self, x1, y1, x2, y2, steps=10):
        """Drags with the primary button, as a user moves a window edge."""
        self.move(x1, y1)
        self.button(True)
        for i in range(1, steps + 1):
            self.move(x1 + (x2 - x1) * i / steps, y1 + (y2 - y1) * i / steps)
        self.button(False)

    def key(self, combo):
        """Presses and releases keys, such as "Return", "ctrl+n" or "Super_L"."""
        self.call("Key", "(s)", [combo])

    def key_down(self, key):
        self.call("KeyDown", "(s)", [key])

    def key_up(self, key):
        self.call("KeyUp", "(s)", [key])

    def type(self, text):
        self.call("Type", "(s)", [text])

    def move(self, x, y):
        self.call("Move", "(ii)", [int(x), int(y)])

    def click(self, x, y, button=1):
        self.call("Click", "(iiu)", [int(x), int(y), button])

    def scroll(self, x, y, direction, clicks=1):
        self.call("Scroll", "(iisu)", [int(x), int(y), direction, clicks])


def real_environment():
    """The environment of the real session, for commands that must not see
    the private one.

    Podman keeps its store under XDG_DATA_HOME and its state under
    XDG_RUNTIME_DIR. With the private values it opens a new, empty store,
    and finds none of the desktops that Cabin made.
    """
    env = {k: v for k, v in os.environ.items() if not k.startswith("XDG_")}
    env["HOME"] = os.environ.get("REAL_HOME", os.environ["HOME"])
    env["XDG_RUNTIME_DIR"] = os.environ.get(
        "REAL_RUNTIME_DIR", f"/run/user/{os.getuid()}"
    )
    return env


def podman(*args, check=True):
    """Runs the podman of the real session, which is the one Cabin talks to."""
    result = subprocess.run(
        ["podman", *args], capture_output=True, text=True, env=real_environment()
    )
    if check and result.returncode != 0:
        raise RuntimeError(f"podman {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def inspect(name):
    """The container's inspect data, or None when it does not exist."""
    out = podman("container", "inspect", name, check=False)
    data = json.loads(out) if out.strip() else []
    return data[0] if data else None


def volumes(prefix):
    out = podman("volume", "ls", "--format", "{{.Name}}")
    return sorted(v for v in out.split() if v.startswith(prefix))


HERE = os.path.dirname(os.path.abspath(__file__))

# Runs in the sandbox. Starts the proxy where Cabin looks for Podman, waits for
# its socket, then becomes Cabin. $1 is the proxy source, $2 the real socket.
LAUNCH = """
mkdir -p "$XDG_RUNTIME_DIR/podman"
python3 -c "$1" "$XDG_RUNTIME_DIR/podman/podman.sock" "$2" &
while [ ! -S "$XDG_RUNTIME_DIR/podman/podman.sock" ]; do sleep 0.1; done
exec cabin
"""


def launch(log_path):
    """Starts the installed Flatpak in the session and returns the process.

    Cabin runs with the permissions of its manifest, behind podman-proxy.py,
    which hides every desktop but the test ones. session.sh puts a hard link
    to the real socket in the private runtime folder, and Flatpak binds
    xdg-run/podman of that folder into the sandbox at its host path.
    """
    with open(os.path.join(HERE, "podman-proxy.py")) as source:
        proxy = source.read()
    real = os.path.join(os.environ["XDG_RUNTIME_DIR"], "podman", "podman.sock")
    log = open(log_path, "w")
    return subprocess.Popen(
        [
            "flatpak",
            "run",
            "--env=RUST_LOG=cabin=debug",
            "--command=sh",
            APP_ID,
            "-c",
            LAUNCH,
            "sh",
            proxy,
            real,
        ],
        stdout=log,
        stderr=subprocess.STDOUT,
    )


def wait_for(predicate, timeout, interval=0.5, what="the condition"):
    """Polls until predicate returns a true value, and returns that value."""
    end = time.monotonic() + timeout
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() > end:
            raise TimeoutError(f"Timed out after {timeout} s waiting for {what}")
        time.sleep(interval)


class Ui:
    """Finds Cabin's widgets on the accessibility bus and clicks them."""

    def __init__(self, shell, app_name="cabin", window_title="Cabin"):
        self.shell = shell
        self.app_name = app_name
        # Dialogues in this application are drawn inside the main window.
        self.window_title = window_title

    def app(self):
        for child in self.apps():
            if (child.get_name() or "").lower() == self.app_name:
                return child
        return None

    def apps(self):
        """Every application on the accessibility bus."""
        desktop = Atspi.get_desktop(0)
        found = []
        for i in range(desktop.get_child_count()):
            child = desktop.get_child_at_index(i)
            if child is not None:
                found.append(child)
        return found

    def find_anywhere(self, name, role=None, timeout=10):
        """A widget in any application, such as the portal's file chooser.

        Its position is relative to its own window, so click it through
        click_in(), which takes that window's title.
        """

        def search():
            for app in self.apps():
                for node in self.nodes(app):
                    try:
                        if node.get_name() == name and (
                            not role or node.get_role_name() == role
                        ):
                            return node
                    except GLib.Error:
                        continue
            return None

        return wait_for(
            search, timeout, what=f"a {role or 'widget'} named {name!r} in any app"
        )

    def click_in(self, node, window_title):
        extents = node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
        fx, fy = self.shell.window(window_title)["frame"][:2]
        self.shell.click(
            fx + extents.x + extents.width / 2, fy + extents.y + extents.height / 2
        )

    def nodes(self, root=None, depth=0):
        """Every accessible under root, depth first, with showing ones only."""
        root = root or self.app()
        if root is None:
            return
        yield root
        if depth > 60:
            return
        try:
            count = root.get_child_count()
        except GLib.Error:
            return
        for i in range(count):
            try:
                child = root.get_child_at_index(i)
            except GLib.Error:
                continue
            if child is None:
                continue
            try:
                states = child.get_state_set()
                if not states.contains(Atspi.StateType.SHOWING):
                    continue
            except GLib.Error:
                continue
            yield from self.nodes(child, depth + 1)

    def find_all(self, name=None, role=None, contains=False):
        found = []
        for node in self.nodes():
            try:
                node_name = node.get_name() or ""
                node_role = node.get_role_name()
            except GLib.Error:
                continue
            if role and node_role != role:
                continue
            if name is not None:
                if contains and name not in node_name:
                    continue
                if not contains and node_name != name:
                    continue
            found.append(node)
        return found

    def find(self, name=None, role=None, contains=False, timeout=10):
        return wait_for(
            lambda: (self.find_all(name, role, contains) or [None])[0],
            timeout,
            what=f"a {role or 'widget'} named {name!r}",
        )

    def dump(self, path):
        """Writes the tree of showing widgets, for working out names."""
        with open(path, "w") as out:
            for node in self.nodes():
                try:
                    where = (
                        "%d,%d %dx%d" % self.rect(node)
                        if node.get_component_iface()
                        else "-"
                    )
                    out.write(f"{node.get_role_name()}\t{node.get_name()!r}\t{where}\n")
                except GLib.Error:
                    pass

    def rect(self, node):
        """The node's rectangle on the screen.

        GTK on Wayland knows positions only inside its own window, so add the
        position of the window frame, which the shell knows.
        """
        extents = node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
        window = self.shell.window(self.window_title)
        fx, fy = window["frame"][:2]
        return fx + extents.x, fy + extents.y, extents.width, extents.height

    def click(self, node):
        """Clicks the middle of node with the virtual pointer.

        Use this outside AdwAlertDialog only. GTK reports the widgets inside
        an alert dialogue some way from where it draws them, so a click there
        can miss. Use activate() and focus() for those.
        """
        x, y, w, h = self.rect(node)
        self.shell.click(x + w / 2, y + h / 2)

    def activate(self, node):
        """Runs the first action of node, as a screen reader does."""
        node.get_action_iface().do_action(0)

    def set_text(self, node, text):
        """Replaces the text of an entry, as a screen reader does.

        GTK does not move the focus over the accessibility bus, so a script
        cannot aim typed keys at an entry in an alert dialogue.
        """
        node.get_editable_text_iface().set_text_contents(text)

    def click_named(self, name, role=None, contains=False, timeout=10):
        node = self.find(name, role, contains, timeout)
        self.click(node)
        return node
