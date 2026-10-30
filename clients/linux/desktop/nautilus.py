# OwnNAS items in the menu of GNOME Files (Nautilus), on files and folders in OwnNAS folders:
# "Always keep on this computer" and "Free up space". Needs nautilus-python
# (python3-nautilus on Debian and Ubuntu). Installed by `ownnas-mount setup`.
import os
import subprocess

import gi

try:
    gi.require_version("Nautilus", "4.0")
except ValueError:
    gi.require_version("Nautilus", "3.0")
from gi.repository import GObject, Nautilus

PROGRAM = "@EXE@"


def state(path):
    """What OwnNAS has on this computer for a path, or None outside OwnNAS folders."""
    try:
        return os.getxattr(path, "user.ownnas.state").decode()
    except OSError:
        return None


class OwnNASMenu(GObject.GObject, Nautilus.MenuProvider):
    def get_file_items(self, *args):
        # Nautilus 43 and later pass (files); earlier ones (window, files).
        files = args[-1]
        paths = [item.get_location().get_path() for item in files]
        if not paths or None in paths:
            return []
        states = [state(path) for path in paths]
        if None in states:
            return []
        items = []
        if any(value != "kept" for value in states):
            keep = Nautilus.MenuItem(name="OwnNAS::keep", label="Always Keep on This Computer")
            keep.connect("activate", lambda *_: subprocess.Popen([PROGRAM, "keep", *paths]))
            items.append(keep)
        if any(value != "online" for value in states):
            free = Nautilus.MenuItem(name="OwnNAS::free", label="Free Up Space")
            free.connect("activate", lambda *_: subprocess.Popen([PROGRAM, "free", *paths]))
            items.append(free)
        return items
