"""Hides every desktop but the test ones from Cabin in the private session.

Usage: podman-proxy.py LISTEN_SOCKET REAL_SOCKET [NAMES [hide-dangling]]

drive.py runs this inside the Flatpak sandbox, beside Cabin, on the socket
path where Cabin looks for Podman.

Cabin in the private session talks to the real Podman of this user, which
also holds the user's own desktops. At start-up Cabin opens the first running
desktop, and the guest serves one RDP session at a time, so a test would take
over a desktop that somebody is using. This proxy adds a name filter to the
desktop list, so Cabin sees only desktops whose names start with "e2e-" or
"shot-". Every other request passes through unchanged. That includes the
unfiltered list that Cabin uses to refuse names and ports in use, so those
checks still see every container.

NAMES replaces the default patterns with a comma separated list of anchored
regular expressions, such as "^fedora-dev$,^web-project$". take.sh uses that
to give its demo desktops natural names. "hide-dangling" also hides untagged
images from the image list, so the Guest Images screenshot does not show the
user's old builds. A prune still sees every image.

Cabin sends one request per connection with "Connection: close", so only the
request line of each connection needs to change.
"""

import asyncio
import json
import os
import sys
from urllib.parse import parse_qsl, quote, urlsplit

PREFIXES = ["^e2e-", "^shot-"]
MANAGED = "io.github.jaydoubleu.cabin.managed=true"
HIDE_DANGLING = False


def rewrite(line):
    """Adds the name filter to a request line that lists Cabin's desktops."""
    try:
        method, target, version = line.decode("latin-1").rstrip("\r\n").split(" ")
    except ValueError:
        return line
    url = urlsplit(target)
    if method == "GET" and HIDE_DANGLING and url.path.endswith("/images/json"):
        filters = json.dumps({"dangling": ["false"]}, separators=(",", ":"))
        target = url.path + "?filters=" + quote(filters, safe="")
        return f"{method} {target} {version}\r\n".encode("latin-1")
    if method != "GET" or not url.path.endswith("/containers/json"):
        return line
    query = parse_qsl(url.query, keep_blank_values=True)
    out = []
    changed = False
    for key, value in query:
        if key == "filters":
            filters = json.loads(value)
            if MANAGED in filters.get("label", []):
                filters["name"] = PREFIXES
                value = json.dumps(filters, separators=(",", ":"))
                changed = True
        out.append(f"{key}={quote(value, safe='')}")
    if not changed:
        return line
    target = url.path + "?" + "&".join(out)
    return f"{method} {target} {version}\r\n".encode("latin-1")


async def pipe(reader, writer, half_close):
    """Copies one direction. A request ends in a half close, so pass that on
    rather than closing, or the answer has nowhere to go."""
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        try:
            if half_close and writer.can_write_eof():
                writer.write_eof()
            else:
                writer.close()
        except (ConnectionError, OSError):
            pass


async def handle(client_reader, client_writer, real):
    try:
        line = await client_reader.readline()
        server_reader, server_writer = await asyncio.open_unix_connection(real)
    except (ConnectionError, OSError):
        client_writer.close()
        return
    server_writer.write(rewrite(line))
    await asyncio.gather(
        pipe(client_reader, server_writer, half_close=True),
        pipe(server_reader, client_writer, half_close=False),
    )
    server_writer.close()


async def main(listen, real):
    if os.path.exists(listen):
        os.unlink(listen)
    server = await asyncio.start_unix_server(
        lambda r, w: handle(r, w, real), path=listen
    )
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    # drive.py passes this source with "python3 -c", so sys.argv[0] is "-c".
    if len(sys.argv) > 3 and sys.argv[3]:
        PREFIXES = sys.argv[3].split(",")
    HIDE_DANGLING = len(sys.argv) > 4 and sys.argv[4] == "hide-dangling"
    asyncio.run(main(sys.argv[1], sys.argv[2]))
