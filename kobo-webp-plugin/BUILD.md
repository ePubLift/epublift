# Building the Kobo WebP plugin from source

A fully reproducible recipe. Everything is built inside a Docker container from
open-source components (Qt + libwebp), so you don't have to trust the prebuilt
binary — and it satisfies the LGPL relink requirement (see
[`LICENSE-NOTES.md`](LICENSE-NOTES.md)).

## Why these exact versions

Kobo's Nickel is built on **Qt 5.2.1** against **glibc 2.19** with a **gcc 4.9**
C++ ABI (`GLIBCXX_3.4.20`). To produce a plugin that loads into Nickel we match
all three:

- **Toolchain:** `arm-nickel-linux-gnueabihf`, **gcc 4.9.4**, **glibc 2.19**,
  ARMv7 hard-float (matches the device).
- **Qt:** cross-build a minimal **qtbase 5.2.1** from `kobolabs/qtbase` (so the
  plugin is version-stamped 5.2.1 and ABI-matches the device).
- **WebP plugin:** the `webp` image-format plugin from **Qt 5.3.0**
  (`qt/qtimageformats`, the first version that has it), which bundles its own
  libwebp — compiled against the 5.2.1 Qt above.

The result needs only `GLIBC_2.4` / `GLIBCXX_3.4`, far below the device's
2.19 / 3.4.20 — so it loads with wide margin.

## Prerequisites

- Docker (the recipe uses a Debian container; on Apple Silicon it runs
  `linux/arm64` natively and is fast).

## 1. Container + build deps

```bash
docker run -d --name kobo-webp-build --platform linux/arm64 \
  -v "$HOME/kobo-webp-build:/work" -w /work debian:bookworm sleep infinity

docker exec kobo-webp-build bash -lc '
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends \
    build-essential autoconf automake bison flex gawk git gperf help2man \
    libncurses-dev libtool libtool-bin make patch pkg-config python3 rsync \
    texinfo unzip wget xz-utils bzip2 cmake curl file ca-certificates lzip
  # crosstool-NG refuses to run as root:
  id builder >/dev/null 2>&1 || useradd -m -s /bin/bash builder
'
```

All subsequent steps run as the **`builder`** user, inside the container's own
ext4 filesystem (not the macOS bind mount — building gcc/glibc on a
case-insensitive FS breaks).

## 2. Cross-toolchain (gcc 4.9.4 / glibc 2.19)

We use [koxtoolchain](https://github.com/koreader/koxtoolchain)'s `nickel`
target, but swap its `gcc-linaro` for vanilla GNU **gcc 4.9.4** — Linaro's old
download server is dead, and vanilla 4.9.4 (from GNU mirrors) is the right ABI
match anyway.

```bash
docker exec kobo-webp-build su - builder -c '
  set -e
  git clone --depth 1 https://github.com/koreader/koxtoolchain.git ~/koxtoolchain
  cd ~/koxtoolchain
  ./gen-tc.sh nickel        # generates build/nickel/.config, then fails at the
                            # gcc-linaro download — expected; we fix the config:
  cd build/nickel
  sed -i "s/^# CT_GCC_USE_GNU is not set/CT_GCC_USE_GNU=y/" .config
  sed -i "s/^CT_GCC_USE_LINARO=y/# CT_GCC_USE_LINARO is not set/" .config
  sed -i "s/^CT_GCC_USE=\"GCC_LINARO\"/CT_GCC_USE=\"GCC\"/" .config
  echo "CT_GCC_V_4_9=y" >> .config
  ../CT_NG_BUILD/bin/ct-ng olddefconfig      # -> CT_GCC_VERSION=4.9.4, glibc 2.19
  ../CT_NG_BUILD/bin/ct-ng build             # ~6 min on Apple Silicon
'
# toolchain -> ~/x-tools/arm-nickel-linux-gnueabihf
```

## 3. Minimal qtbase 5.2.1 (cross)

```bash
docker exec kobo-webp-build su - builder -c '
  set -e
  TC=arm-nickel-linux-gnueabihf
  export PATH="$HOME/x-tools/$TC/bin:$PATH"
  SYSROOT="$HOME/x-tools/$TC/$TC/sysroot"
  git clone --depth 1 https://github.com/kobolabs/qtbase.git ~/qtbase
  cd ~/qtbase

  # minimal mkspec that uses our toolchain (no /chroot device-sysroot deps)
  SPEC=mkspecs/linux-armv7-kobo-min-g++
  cp -r mkspecs/linux-armv7-kobo-g++ "$SPEC"
  cat > "$SPEC/qmake.conf" <<EOF
MAKEFILE_GENERATOR = UNIX
CONFIG += incremental
QMAKE_INCREMENTAL_STYLE = sublib
include(../common/linux.conf)
include(../common/gcc-base-unix.conf)
include(../common/g++-unix.conf)
QMAKE_CFLAGS_RELEASE   = -O2 -march=armv7-a -mfpu=neon -mfloat-abi=hard -fPIC
QMAKE_CXXFLAGS_RELEASE = \$\$QMAKE_CFLAGS_RELEASE -std=c++11
QMAKE_CC  = ${TC}-gcc
QMAKE_CXX = ${TC}-g++
QMAKE_LINK = ${TC}-g++
QMAKE_LINK_SHLIB = ${TC}-g++
QMAKE_AR = ${TC}-ar cqs
QMAKE_OBJCOPY = ${TC}-objcopy
QMAKE_NM = ${TC}-nm -P
QMAKE_STRIP = ${TC}-strip
load(qt_config)
EOF

  ./configure -prefix /home/builder/qt5kobo-min -release -opensource -confirm-license \
    -xplatform linux-armv7-kobo-min-g++ -sysroot "$SYSROOT" \
    -no-pch -no-pkg-config \
    -qt-zlib -qt-libpng -qt-pcre -qt-freetype \
    -no-libjpeg -no-icu -no-glib -no-dbus -no-openssl \
    -no-opengl -no-egl -no-eglfs -no-xcb -no-directfb -no-fontconfig -no-cups -no-harfbuzz \
    -nomake examples -nomake tests
  # -no-pch is REQUIRED (cross PCH is invalid and fails the build)

  make -j"$(nproc)" qmake_all
  cd src && make -j"$(nproc)" sub-corelib && make -j"$(nproc)" sub-gui
  # -> ~/qtbase/lib/libQt5Core.so.5.2.1, libQt5Gui.so.5.2.1, ~/qtbase/bin/moc
'
```

## 4. The WebP plugin (from Qt 5.3.0, against 5.2.1)

```bash
docker exec kobo-webp-build su - builder -c '
  set -e
  TC=arm-nickel-linux-gnueabihf
  export PATH="$HOME/x-tools/$TC/bin:$PATH"
  git clone --depth 1 --branch v5.3.0 https://github.com/qt/qtimageformats.git ~/qtimageformats
  cd ~/qtimageformats/src/plugins/imageformats/webp
  "$HOME/qtbase/bin/qmake" -spec linux-armv7-kobo-min-g++
  make -j"$(nproc)"
  # -> ~/qtimageformats/plugins/imageformats/libqwebp.so
  "$HOME/x-tools/$TC/bin/$TC-strip" --strip-unneeded \
    ~/qtimageformats/plugins/imageformats/libqwebp.so
'
```

## 5. Verify + package

```bash
docker exec kobo-webp-build su - builder -c '
  TC=arm-nickel-linux-gnueabihf
  SO=~/qtimageformats/plugins/imageformats/libqwebp.so
  # must need only GLIBC_2.4 / GLIBCXX_3.4 (device has 2.19 / 3.4.20):
  ~/x-tools/$TC/bin/$TC-objdump -T "$SO" | grep -oE "GLIBC_[0-9.]+|GLIBCXX_[0-9.]+" | sort -V | uniq
'
docker exec kobo-webp-build bash -lc '
  rm -rf /tmp/kr && mkdir -p /tmp/kr/usr/local/Kobo/imageformats
  cp ~builder/qtimageformats/plugins/imageformats/libqwebp.so /tmp/kr/usr/local/Kobo/imageformats/
  cd /tmp/kr && tar czf /work/KoboRoot.tgz --owner=0 --group=0 usr
'
```

`KoboRoot.tgz` lands in `~/kobo-webp-build/` on the host. Install per
[`INSTALL.md`](INSTALL.md).

## Notes

- If the plugin builds but **fails to load** on your device, the most likely
  cause is a `qreal` ABI mismatch — Qt 5.2 defaults `qreal` to `double`; if your
  firmware's Qt was configured `-qreal float`, rebuild qtbase (step 3) with
  `-qreal float` and redo step 4.
- Built and verified against **Forma (N782), fw 4.38.23697**. Other models share
  the same Qt 5.2.1 base, but verify on your own device.
