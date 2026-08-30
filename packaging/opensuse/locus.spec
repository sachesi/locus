%define _debugsource_template %{nil}
%define debug_package %{nil}

%global app_id io.github.sachesi.locus

Name:           locus
# The release workflow sets Version to the tag it builds; OBS counts the Release.
Version:        0.1.0
Release:        0
Summary:        Watch processes and what they use

License:        GPL-3.0-or-later
URL:            https://github.com/sachesi/locus
# Named as the Debian source package names them, which OBS builds from the same files.
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}_%{version}.orig.tar.gz
# The crates the build needs, from the release, so that it runs without a network.
Source1:        %{url}/releases/download/v%{version}/%{name}-%{version}-vendor.tar.xz#/%{name}_%{version}.orig-vendor.tar.xz

BuildRequires:  cargo
BuildRequires:  rust >= 1.92
BuildRequires:  gcc
BuildRequires:  blueprint-compiler
BuildRequires:  desktop-file-utils
BuildRequires:  gettext-tools
BuildRequires:  AppStream
BuildRequires:  pkgconfig(gtk4) >= 4.22
BuildRequires:  pkgconfig(libadwaita-1) >= 1.8

Requires:       libgtk-4-1 >= 4.22
Requires:       libadwaita-1-0 >= 1.8
Requires:       hicolor-icon-theme

%description
Locus is a system monitor for Wayland desktops, built with GTK 4 and
libadwaita. It lists the running processes with the share of the processor,
the memory and the disk traffic of each, and ends, kills, stops and continues
them; it graphs the processor and each of its cores, memory and swap, the
network and the disks, and shows the room left on each mounted disk.

Everything is read from /proc and /sys, and processes are read only while
their list is on screen.

%prep
%autosetup -n %{name}-%{version} -b 1

%build
export CARGO_HOME="$PWD/.cargo-home"
export RUSTFLAGS="%{?build_rustflags}"
export LOCUS_LOCALEDIR="%{_datadir}/locale"
%if 0%{?_cargo_target_dir:1}
export CARGO_TARGET_DIR="%{_cargo_target_dir}"
%endif
cargo build --release --offline --locked

%install
%if 0%{?_cargo_target_dir:1}
target="%{_cargo_target_dir}/release"
%else
target="target/release"
%endif
install -Dpm 0755 "$target/locus" %{buildroot}%{_bindir}/locus

install -d %{buildroot}%{_datadir}/applications %{buildroot}%{_datadir}/metainfo
msgfmt --desktop --template=data/%{app_id}.desktop -d po \
  -o %{buildroot}%{_datadir}/applications/%{app_id}.desktop
msgfmt --xml --template=data/%{app_id}.metainfo.xml -d po \
  -o %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
install -Dpm 0644 data/%{app_id}.gschema.xml \
  %{buildroot}%{_datadir}/glib-2.0/schemas/%{app_id}.gschema.xml
install -Dpm 0644 data/icons/hicolor/scalable/apps/%{app_id}.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
install -Dpm 0644 data/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg \
  %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

for lang in $(cat po/LINGUAS); do
  install -d %{buildroot}%{_datadir}/locale/$lang/LC_MESSAGES
  msgfmt -o %{buildroot}%{_datadir}/locale/$lang/LC_MESSAGES/%{name}.mo po/$lang.po
done
%find_lang %{name}

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
test -x %{buildroot}%{_bindir}/locus

%files -f %{name}.lang
%license LICENSE
%doc README.md
%{_bindir}/locus
%{_datadir}/applications/%{app_id}.desktop
%{_datadir}/metainfo/%{app_id}.metainfo.xml
%{_datadir}/glib-2.0/schemas/%{app_id}.gschema.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

%changelog
