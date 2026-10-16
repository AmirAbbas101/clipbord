Name:           clipbord
Version:        0.1.0
Release:        1%{?dist}
Summary:        Clipboard history for Fedora / GNOME (Win+V style)

License:        MIT
URL:            https://github.com/example/clipbord
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  pkgconf-pkg-config
BuildRequires:  gtk4-devel
BuildRequires:  libadwaita-devel
BuildRequires:  openssl-devel

Requires:       gtk4
Requires:       libadwaita
Requires:       wl-clipboard
Recommends:     wtype

%description
Clipbord is a fast, memory-capped clipboard history manager for Fedora
Workstation (GNOME / Wayland). It provides a Win+V / macOS-like history
popup with search, pinning, keyboard navigation, and a background daemon.

%prep
%autosetup

%build
cargo build --release --locked

%install
install -D -m 755 target/release/clipbord %{buildroot}%{_bindir}/clipbord
install -D -m 644 packaging/io.github.clipbord.Clipbord.desktop \
  %{buildroot}%{_datadir}/applications/io.github.clipbord.Clipbord.desktop
install -D -m 644 packaging/clipbord.system.service \
  %{buildroot}%{_userunitdir}/clipbord.service
install -D -m 644 README.md %{buildroot}%{_pkgdocdir}/README.md

%files
%license LICENSE
%doc README.md
%{_bindir}/clipbord
%{_datadir}/applications/io.github.clipbord.Clipbord.desktop
%{_userunitdir}/clipbord.service
%{_pkgdocdir}/README.md

%changelog
* Tue Oct 14 2026 Clipbord Contributors <dev@example.com> - 0.1.0-1
- Initial package
