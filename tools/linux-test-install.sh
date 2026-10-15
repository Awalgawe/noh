#!/bin/sh
# Disposable test-image setup only. This never installs packages on the host.
set -eu
test -f /.dockerenv || test -f /run/.containerenv || test "${NOH_CONTAINER_BUILD:-}" = 1
case "$1" in
  ubuntu-24.04|ubuntu-26.04|debian-13)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update
    apt-get install --no-install-recommends -y python3 ffmpeg libmpv2 \
      libasound2-plugins libgl1-mesa-dri libegl1 libxkbcommon-x11-0 \
      libxcursor1 libxrandr2 libxi6 libxinerama1 libstdc++6 fonts-dejavu-core \
      dbus-x11 xvfb xauth weston
    if test "$1" = ubuntu-26.04; then
      apt-get install --no-install-recommends -y pipewire pipewire-alsa wireplumber
    else
      apt-get install --no-install-recommends -y pulseaudio pulseaudio-utils
    fi
    dpkg-query -W > /opt/noh/packages.txt
    ;;
  fedora-44)
    dnf install -y https://download1.rpmfusion.org/free/fedora/rpmfusion-free-release-44.noarch.rpm
    dnf install -y --allowerasing ffmpeg mpv-libs python3 pipewire pipewire-utils pipewire-alsa \
      wireplumber mesa-dri-drivers mesa-libEGL mesa-libGL libxkbcommon-x11 \
      libXcursor libXrandr libXi libXinerama libstdc++ dbus-daemon weston \
      dejavu-sans-fonts shadow-utils
    rpm -qa --qf '%{NAME} %{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n' | sort > /opt/noh/packages.txt
    ;;
  opensuse-tumbleweed)
    zypper --non-interactive refresh
    zypper --non-interactive install --no-recommends python3 ffmpeg-8 libmpv2 \
      alsa pipewire pipewire-alsa wireplumber Mesa-dri Mesa-libEGL1 libglvnd \
      libxkbcommon-x11-0 libXcursor1 libXrandr2 libXi6 libXinerama1 \
      xorg-x11-server-Xvfb dbus-1 dbus-1-daemon dejavu-fonts shadow
    # Full codec support is supplied by the distribution-compatible Packman provider.
    zypper --non-interactive addrepo --refresh --priority 90 \
      https://ftp.gwdg.de/pub/linux/misc/packman/suse/openSUSE_Tumbleweed/Essentials/ packman-essentials
    zypper --non-interactive --gpg-auto-import-keys refresh packman-essentials
    zypper --non-interactive dist-upgrade --from packman-essentials --allow-vendor-change
    rpm -qa --qf '%{NAME} %{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n' | sort > /opt/noh/packages.txt
    ;;
  arch)
    pacman -Syu --noconfirm --needed ffmpeg mpv python alsa-lib pipewire \
      pipewire-audio pipewire-alsa wireplumber mesa libglvnd libxkbcommon-x11 \
      libxcursor libxrandr libxi libxinerama dbus weston ttf-dejavu shadow
    pacman -Q > /opt/noh/packages.txt
    ;;
  *) echo 'Unsupported Linux test case' >&2; exit 2 ;;
esac
if ! getent passwd 1000 >/dev/null; then useradd --uid 1000 --create-home nohprobe; fi
