# enso-files as the desktop's file chooser

Every application that asks the desktop portal for a file — Firefox,
Chromium, Electron apps, Flatpaks — can get `enso-files pick` instead of
GTK's chooser. Four pieces, all in your home directory, no root:

1. **The service**: `~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.enso.service`,
   so D-Bus starts `enso-files --service` when it is first asked:

   ```ini
   [D-BUS Service]
   Name=org.freedesktop.impl.portal.desktop.enso
   Exec=/home/you/.local/bin/enso-files --service
   ```

2. **The portal file**, [`enso.portal`](enso.portal), which tells
   xdg-desktop-portal what the service does. xdg-desktop-portal reads such
   files from one directory: `/usr/share/xdg-desktop-portal/portals`, or
   `$XDG_DESKTOP_PORTAL_DIR`. So a directory of your own,
   `~/.local/share/enso/portals`, holds `enso.portal` and a link to every
   system `.portal` file, and

3. **a drop-in** for the portal's user service points it there,
   `~/.config/systemd/user/xdg-desktop-portal.service.d/enso.conf`:

   ```ini
   [Service]
   Environment=XDG_DESKTOP_PORTAL_DIR=%h/.local/share/enso/portals
   ```

4. **The routing**, `niri-portals.conf` — in that same directory, since
   with `XDG_DESKTOP_PORTAL_DIR` set the portal reads its configuration
   from there too: the system's file, plus one line sending file choosers
   to enso:

   ```ini
   [preferred]
   default=gnome;gtk;
   org.freedesktop.impl.portal.FileChooser=enso
   org.freedesktop.impl.portal.Access=gtk;
   org.freedesktop.impl.portal.Notification=gtk;
   org.freedesktop.impl.portal.Secret=gnome-keyring;
   ```

Then `systemctl --user daemon-reload && systemctl --user restart
xdg-desktop-portal`. sumi's `install --enso` does all of this.

**To go back to GTK's chooser**, delete the `FileChooser=enso` line and
restart `xdg-desktop-portal`.

The system's `.portal` files change when packages do; running the install
again refreshes the links.
