# Installing the Kobo WebP plugin

> Read the [Disclaimer](README.md#disclaimer) first. You do this at your own risk.

## 1. Install

1. Connect your Kobo to a computer over USB. The `KOBOeReader` drive appears.
2. At the **root** of that drive there is a hidden folder named **`.kobo`**
   (enable "show hidden files" if you can't see it).
3. Copy **`KoboRoot.tgz`** directly into that `.kobo` folder (not into a
   subfolder).
4. **Eject / safely remove** the Kobo and unplug it.
5. The Kobo shows **"Updating…"** and reboots. On boot, Nickel extracts
   `KoboRoot.tgz` to the system partition, placing the plugin at
   `/usr/local/Kobo/imageformats/libqwebp.so`, then deletes the `.tgz`.

**Confirm it installed:** reconnect over USB — if `KoboRoot.tgz` is **gone** from
`.kobo`, the install succeeded.

## 2. Test

WebP only works through the **`.kepub.epub`** path (see
[README](README.md#what-it-fixes-and-what-it-doesnt)), so test with a `.kepub.epub`:

1. Produce a Kobo file with WebP images, e.g. with ePubLift:
   ```
   epublift -i book.epub --kepub --webp     # see note below
   ```
2. Copy the `.kepub.epub` onto the Kobo (into `.kobo/kepub/` via Calibre, or to
   the drive root).
3. On the home screen the **cover** should now render, and opening the book the
   **in-book images** should render.

> **Note on `--webp` + `--kepub`:** historically ePubLift forced original images
> for `--kepub` (because stock Kobo can't show WebP). With this plugin installed
> you can opt back into WebP. If your ePubLift build doesn't yet expose that
> opt-in, just convert normally to a plain `_v3.3.epub` (WebP) and rename/convert
> it to `.kepub.epub`, or use a build that supports the flag.

### Stale cover thumbnails

Kobo caches cover thumbnails in a hidden **`.kobo-images`** folder at the drive
root. Books that were on the device **before** you installed the plugin keep
their old (blank) cached cover. To refresh:

- remove and re-add the book, **or**
- delete the contents of `.kobo-images` (Kobo regenerates all covers on next
  use — may take a while for a large library).

A **freshly imported** book always gets a fresh thumbnail, so it's the cleanest
way to verify.

## 3. Uninstall

Delete the plugin file from the device:

```
/usr/local/Kobo/imageformats/libqwebp.so
```

You'll need shell access (e.g. via an SSH/telnet mod) to remove it directly, or
a factory reset / firmware re-flash will clear it. Removing it simply returns
Kobo to its stock behaviour (WebP blank again); nothing else depends on it.

## If it doesn't work

- **Covers of old books still blank** → stale `.kobo-images` cache (see above);
  test with a brand-new imported book.
- **Plain `.epub` cover still blank** → expected; that path isn't fixable by this
  plugin. Use `.kepub.epub`.
- **Nothing renders even in kepub** → the plugin may not have loaded (an
  incompatible build is silently ignored). Re-check the install, and consider
  building from source for your exact firmware ([`BUILD.md`](BUILD.md)).
