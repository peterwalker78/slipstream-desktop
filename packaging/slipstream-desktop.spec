# Slipstream for Fedora (COPR) and openSUSE (the openSUSE Build Service). Built from the source
# tarball each release carries, which holds every crate it needs, so it builds offline.
# packaging/prepare fills in the version.

%global upstream_version @VERSION@
# Release builds are stripped, so there is no debug information to split out.
%global debug_package %{nil}

Name:           slipstream-desktop
Version:        @RPM_VERSION@
Release:        1%{?dist}
Summary:        Keyboard-first Wayland desktop session
License:        GPL-3.0-or-later AND MIT AND Apache-2.0 AND OFL-1.1
URL:            https://github.com/peterwalker78/slipstream-desktop
Source0:        %{url}/releases/download/v%{upstream_version}/slipstream-desktop-%{upstream_version}.tar.xz
ExclusiveArch:  x86_64

BuildRequires:  cargo >= 1.92
BuildRequires:  rust >= 1.92
BuildRequires:  clang
BuildRequires:  mold
BuildRequires:  pkgconfig
BuildRequires:  pkgconfig(gbm)
BuildRequires:  pkgconfig(egl)
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libdisplay-info)
BuildRequires:  pkgconfig(libdrm)
BuildRequires:  pkgconfig(libinput)
BuildRequires:  pkgconfig(libseat)
BuildRequires:  pkgconfig(libsystemd)
BuildRequires:  pkgconfig(libudev)
BuildRequires:  pkgconfig(pixman-1)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(wayland-server)
BuildRequires:  pkgconfig(xkbcommon)

# Opened while it runs rather than linked: drawing, and the lock screen's password check.
Requires:       libEGL.so.1()(64bit)
Requires:       libpam.so.0()(64bit)

# Each of these is one feature; the session runs without any of them.
Recommends:     xdg-desktop-portal
Recommends:     xdg-desktop-portal-wlr
Recommends:     wireplumber
Recommends:     pulseaudio-utils
Recommends:     NetworkManager
Recommends:     bluez
Recommends:     power-profiles-daemon
Recommends:     bubblewrap
%if 0%{?suse_version}
Recommends:     xwayland
Recommends:     pipewire-tools
%else
Recommends:     xorg-x11-server-Xwayland
Recommends:     pipewire-utils
%endif

%description
Slipstream is a keyboard-first Wayland desktop session, with tiling, workspaces and a settings
app of its own. It installs alongside the desktop you use now: pick Slipstream from the list on
the login screen.

%prep
%autosetup -n slipstream-desktop-%{upstream_version}

%build
# The build follows the workspace's release profile and .cargo/config.toml, as the download does;
# a distribution's RUSTFLAGS would replace both.
env -u RUSTFLAGS cargo build --release --locked --offline -p slipstream -p slipstream-settings -p slipstream-focus

%install
%if 0%{?suse_version}
sh packaging/install-files %{buildroot} suse
%else
sh packaging/install-files %{buildroot} fedora
%endif

%post
if [ "$1" -eq 1 ]; then
    echo "Slipstream is on your login screen's list of sessions now."
    echo "Glimmerwood, a browser, and Westering, a few quiet minutes under the night sky, go well"
    echo "with it. They aren't part of this package: github.com/peterwalker78/glimmerwood and"
    echo "github.com/peterwalker78/westering"
fi

%files
%{_datadir}/licenses/%{name}/
%{_bindir}/slipstream
%{_bindir}/slipstream-settings
%{_bindir}/slipstream-session
%{_datadir}/applications/io.github.peterwalker78.SlipstreamSettings.desktop
%dir %{_datadir}/wayland-sessions
%{_datadir}/wayland-sessions/slipstream.desktop
%dir %{_prefix}/lib/systemd
%dir %{_prefix}/lib/systemd/user
%{_prefix}/lib/systemd/user/slipstream-*.target
%{_datadir}/slipstream/
%dir %{_datadir}/xdg-desktop-portal
%dir %{_datadir}/xdg-desktop-portal/portals
%{_datadir}/xdg-desktop-portal/portals/slipstream.portal
%{_datadir}/xdg-desktop-portal/slipstream-portals.conf
%dir %{_sysconfdir}/xdg/xdg-desktop-portal-wlr
%config(noreplace) %{_sysconfdir}/xdg/xdg-desktop-portal-wlr/Slipstream
%if 0%{?suse_version}
%{_prefix}/lib/pam.d/slipstream
%else
%config(noreplace) %{_sysconfdir}/pam.d/slipstream
%endif

%changelog
* @CHANGELOG_DATE@ Slipstream <4011711+peterwalker78@users.noreply.github.com> - @RPM_VERSION@-1
- Slipstream @VERSION@
