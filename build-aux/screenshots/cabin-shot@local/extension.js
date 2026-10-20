// Test only. Takes screenshots, reads window positions and sends real input
// on request over D-Bus, for the scripts in build-aux/screenshots.
//
// GNOME Shell does not let other clients use its own screenshot interface,
// and a Wayland client cannot inject input into another client. A virtual
// device that the compositor owns goes through the same path as a physical
// keyboard and mouse, so the input path of the application is tested too.
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const NAME = 'io.github.jaydoubleu.CabinShot';
const PATH = '/io/github/jaydoubleu/CabinShot';

const XML = `<node><interface name="${NAME}">
  <method name="Shot"><arg type="s" name="path" direction="in"/></method>
  <method name="ShotWindow"><arg type="s" name="path" direction="in"/></method>
  <method name="ShotArea">
    <arg type="s" name="path" direction="in"/>
    <arg type="i" name="x" direction="in"/><arg type="i" name="y" direction="in"/>
    <arg type="i" name="width" direction="in"/><arg type="i" name="height" direction="in"/>
  </method>
  <method name="HideOverview"/>
  <method name="OverviewVisible"><arg type="b" name="visible" direction="out"/></method>
  <method name="ModalCount"><arg type="i" name="count" direction="out"/></method>
  <method name="ListWindows"><arg type="s" name="json" direction="out"/></method>
  <method name="Activate">
    <arg type="s" name="title" direction="in"/><arg type="b" name="found" direction="out"/>
  </method>
  <method name="Maximize">
    <arg type="s" name="title" direction="in"/><arg type="b" name="found" direction="out"/>
  </method>
  <method name="Key"><arg type="s" name="combo" direction="in"/></method>
  <method name="KeyDown"><arg type="s" name="key" direction="in"/></method>
  <method name="KeyUp"><arg type="s" name="key" direction="in"/></method>
  <method name="Type"><arg type="s" name="text" direction="in"/></method>
  <method name="Move">
    <arg type="i" name="x" direction="in"/><arg type="i" name="y" direction="in"/>
  </method>
  <method name="Click">
    <arg type="i" name="x" direction="in"/><arg type="i" name="y" direction="in"/>
    <arg type="u" name="button" direction="in"/>
  </method>
  <method name="Button">
    <arg type="u" name="button" direction="in"/><arg type="b" name="pressed" direction="in"/>
  </method>
  <method name="Scroll">
    <arg type="i" name="x" direction="in"/><arg type="i" name="y" direction="in"/>
    <arg type="s" name="direction" direction="in"/><arg type="u" name="clicks" direction="in"/>
  </method>
</interface></node>`;

// The gap between two input events. Without one, a client that reads input
// once per frame can see a press and its release as the same moment.
const GAP_MS = 20;

// Runs one Shell.Screenshot method into a PNG file and answers the D-Bus call.
function shoot(path, invocation, method, finish, args) {
    const file = Gio.File.new_for_path(path);
    const stream = file.replace(null, false, Gio.FileCreateFlags.NONE, null);
    const shooter = new Shell.Screenshot();
    shooter[method](...args, stream, (_shooter, result) => {
        try {
            shooter[finish](result);
            stream.close(null);
            invocation.return_value(null);
        } catch (e) {
            invocation.return_error_literal(Gio.DBusError, Gio.DBusError.FAILED, `${e}`);
        }
    });
}

// A key name such as "Return", "Super_L", "F11" or "a" to a keyval.
function keyval(name) {
    const named = Clutter[`KEY_${name}`];
    if (named !== undefined)
        return named;
    if ([...name].length === 1)
        return Clutter.unicode_to_keysym(name.codePointAt(0));
    throw new Error(`Unknown key name: ${name}`);
}

// Short names for the modifiers, so a combination reads like "ctrl+n".
const ALIASES = {
    ctrl: 'Control_L', control: 'Control_L', shift: 'Shift_L', alt: 'Alt_L',
    super: 'Super_L', enter: 'Return', esc: 'Escape', space: 'space',
};

function keyvalOf(part) {
    return keyval(ALIASES[part.toLowerCase()] ?? part);
}

// Application windows and their dialogues. A modal dialogue that belongs to
// another window, such as the portal's file chooser, skips the taskbar, so
// that flag cannot be the test.
const LISTED_TYPES = [
    Meta.WindowType.NORMAL, Meta.WindowType.DIALOG, Meta.WindowType.MODAL_DIALOG,
];

function windows() {
    return global.get_window_actors()
        .map(actor => actor.get_meta_window())
        .filter(window => window && LISTED_TYPES.includes(window.get_window_type()));
}

function findWindow(title) {
    return windows().find(window => (window.get_title() ?? '').includes(title)) ?? null;
}

export default class CabinShot extends Extension {
    enable() {
        const seat = Clutter.get_default_backend().get_default_seat();
        this._keyboard = seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
        this._pointer = seat.create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE);
        this._impl = Gio.DBusExportedObject.wrapJSObject(XML, {
            ShotAsync: ([path], invocation) =>
                shoot(path, invocation, 'screenshot', 'screenshot_finish', [false]),
            ShotWindowAsync: ([path], invocation) =>
                shoot(path, invocation, 'screenshot_window', 'screenshot_window_finish', [true, false]),
            ShotAreaAsync: ([path, x, y, width, height], invocation) =>
                shoot(path, invocation, 'screenshot_area', 'screenshot_area_finish', [x, y, width, height]),
            HideOverview: () => Main.overview.hide(),
            // The overview of this shell, not of a guest drawn in a window.
            // A key that should reach a guest must leave this false.
            OverviewVisible: () => Main.overview.visible,
            // Above zero while the shell shows a modal dialogue of its own,
            // such as the one that asks to allow inhibiting shortcuts.
            ModalCount: () => Main.modalCount,
            ListWindows: () => JSON.stringify(windows().map(window => {
                const frame = window.get_frame_rect();
                const buffer = window.get_buffer_rect();
                return {
                    title: window.get_title(),
                    wm_class: window.get_wm_class(),
                    focused: window.has_focus(),
                    maximized: window.is_maximized(),
                    fullscreen: window.is_fullscreen(),
                    frame: [frame.x, frame.y, frame.width, frame.height],
                    buffer: [buffer.x, buffer.y, buffer.width, buffer.height],
                };
            })),
            Activate: ([title]) => {
                const window = findWindow(title);
                if (window)
                    window.activate(global.get_current_time());
                return window !== null;
            },
            // Only maximise. Unmaximise and move_resize_frame() from here
            // leave a client side decorated window where it was, so a script
            // restores or resizes a window as a user does: with the pointer.
            Maximize: ([title]) => {
                const window = findWindow(title);
                if (window)
                    window.maximize();
                return window !== null;
            },
            KeyAsync: ([combo], invocation) => {
                const vals = combo.split('+').map(keyvalOf);
                const steps = vals.map(v => () => this._key(v, true));
                vals.slice().reverse().forEach(v => steps.push(() => this._key(v, false)));
                this._run(steps, invocation);
            },
            KeyDownAsync: ([key], invocation) =>
                this._run([() => this._key(keyvalOf(key), true)], invocation),
            KeyUpAsync: ([key], invocation) =>
                this._run([() => this._key(keyvalOf(key), false)], invocation),
            TypeAsync: ([text], invocation) => {
                const steps = [];
                for (const ch of text) {
                    const v = ch === '\n' ? Clutter.KEY_Return : keyval(ch);
                    steps.push(() => this._key(v, true), () => this._key(v, false));
                }
                this._run(steps, invocation);
            },
            MoveAsync: ([x, y], invocation) =>
                this._run([() => this._pointer.notify_absolute_motion(this._now(), x, y)], invocation),
            ClickAsync: ([x, y, button], invocation) => this._run([
                () => this._pointer.notify_absolute_motion(this._now(), x, y),
                () => this._pointer.notify_button(this._now(), button || 1, Clutter.ButtonState.PRESSED),
                () => this._pointer.notify_button(this._now(), button || 1, Clutter.ButtonState.RELEASED),
            ], invocation),
            ButtonAsync: ([button, pressed], invocation) => this._run([
                () => this._pointer.notify_button(this._now(), button || 1,
                    pressed ? Clutter.ButtonState.PRESSED : Clutter.ButtonState.RELEASED),
            ], invocation),
            ScrollAsync: ([x, y, direction, clicks], invocation) => {
                const dir = {
                    up: Clutter.ScrollDirection.UP, down: Clutter.ScrollDirection.DOWN,
                    left: Clutter.ScrollDirection.LEFT, right: Clutter.ScrollDirection.RIGHT,
                }[direction];
                if (dir === undefined) {
                    invocation.return_error_literal(Gio.DBusError, Gio.DBusError.INVALID_ARGS,
                        `Unknown scroll direction: ${direction}`);
                    return;
                }
                const steps = [() => this._pointer.notify_absolute_motion(this._now(), x, y)];
                for (let i = 0; i < Math.max(clicks, 1); i++) {
                    steps.push(() => this._pointer.notify_discrete_scroll(
                        this._now(), dir, Clutter.ScrollSource.WHEEL));
                }
                this._run(steps, invocation);
            },
        });
        this._impl.export(Gio.DBus.session, PATH);
        this._owner = Gio.bus_own_name(Gio.BusType.SESSION, NAME,
            Gio.BusNameOwnerFlags.NONE, null, null, null);
    }

    disable() {
        this._impl.unexport();
        Gio.bus_unown_name(this._owner);
        this._keyboard = null;
        this._pointer = null;
    }

    _now() {
        return GLib.get_monotonic_time();
    }

    _key(value, pressed) {
        this._keyboard.notify_keyval(this._now(), value,
            pressed ? Clutter.KeyState.PRESSED : Clutter.KeyState.RELEASED);
    }

    // Runs input steps one after another with a gap, then answers the call.
    _run(steps, invocation) {
        try {
            if (steps.length === 0) {
                invocation.return_value(null);
                return;
            }
            steps[0]();
        } catch (e) {
            invocation.return_error_literal(Gio.DBusError, Gio.DBusError.FAILED, `${e}`);
            return;
        }
        GLib.timeout_add(GLib.PRIORITY_DEFAULT, GAP_MS, () => {
            this._run(steps.slice(1), invocation);
            return GLib.SOURCE_REMOVE;
        });
    }
}
