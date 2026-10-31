# Packaging

`npx tauri build` (in `apps/desktop`) builds the release binary and three Linux packages under `target/release/bundle/`:

| Package | File | For |
|---|---|---|
| Debian package | `deb/universal-calendar_<version>_amd64.deb` | Debian, Ubuntu, KDE neon, Kubuntu and derivatives |
| RPM | `rpm/universal-calendar-<version>-1.x86_64.rpm` | Fedora, openSUSE and other RPM distributions |
| AppImage | `appimage/universal-calendar_<version>_amd64.AppImage` | Any recent x86-64 distribution, without installing |

The configuration is in `apps/desktop/src-tauri/tauri.conf.json` (`bundle`) and `apps/desktop/src-tauri/linux/`.

## What the packages install

- `/usr/bin/universal-calendar`: the application.
- `/usr/share/applications/universal-calendar.desktop`: the menu entry, from the template `linux/universal-calendar.desktop`. It has category Office/Calendar, search keywords, and a `StartupWMClass` matching the window, so Plasma's task manager groups the window with its launcher.
- `/usr/share/icons/hicolor/{32,64,128,256,512}/apps/universal-calendar.png`.
- `/usr/share/metainfo/org.universalcalendar.desktop.metainfo.xml`: AppStream metadata, used by Discover and GNOME Software. It passes `appstreamcli validate --pedantic`.

The AppImage contains the same files and its own copies of GTK, WebKitGTK and their libraries, about 85 MB.

## Dependencies

- **deb:** `libwebkit2gtk-4.1-0` and `libgtk-3-0`; apt installs the rest.
- **rpm:** `libwebkit2gtk-4.1.so.0` and `libgtk-3.so.0` (by soname), so it installs on any RPM distribution that ships WebKitGTK 4.1.
- TLS is built in (rustls), so there is no OpenSSL dependency.

At runtime, without being a package dependency:

- **Secret Service** (KWallet on Plasma, GNOME Keyring elsewhere) for account passwords and sign-in tokens. Without it, adding an account fails with a message.
- **A notification service** (every desktop has one) for reminders.

## Testing the packages

`scripts/test-packages.sh` installs each package in a clean Docker container, using the distribution's own package manager to pull dependencies. It then:

- validates the desktop entry (`desktop-file-validate`) and the AppStream metadata (`appstreamcli`);
- starts the app in a virtual display with a fresh profile;
- checks it is still running after 12 seconds, and saves a screenshot.

| Container | Package |
|---|---|
| Ubuntu 24.04 | deb |
| Debian 12 | deb |
| Fedora 41 | rpm |
| Debian 12 **without WebKit** | AppImage, which has to bring everything it needs |

```sh
(cd apps/desktop && npx tauri build)
scripts/test-packages.sh            # results in target/package-test/
```

It needs Docker and downloads the base images and packages, several hundred MB on the first run.

## Names and identifiers

| What | Value |
|---|---|
| Package, binary, icon and desktop file | `universal-calendar` |
| Application identifier (data directory, keyring service) | `org.universalcalendar.desktop` |
| AppStream id | `org.universalcalendar.desktop` |
| Window title and menu name | Universal Calendar |

`productName` in `tauri.conf.json` is `universal-calendar` because Tauri names the desktop file after it. The visible name comes from the window title and the desktop template. Renaming the binary from the earlier `universal-calendar-desktop` didn't move any data: the data directory and keyring entries use the application identifier.

## Not yet done

- **Flatpak.** A manifest needs `flatpak-builder` and the GNOME runtime (which provides WebKitGTK 4.1) to build and test, and neither is available in this development VM. The likely route is to build from source in the manifest, with vendored Cargo and npm dependencies.
- **Signing and repositories:** packages aren't signed, and there is no APT or DNF repository.
- **Screenshots in the AppStream metadata.** Software centres show them, but they need stable public image URLs.
- **The Debian `Maintainer` field** has no e-mail address, which `lintian` would flag. A distributor would add theirs.
- **ARM64** packages aren't built (cross-building WebKitGTK apps needs an ARM64 build host).
