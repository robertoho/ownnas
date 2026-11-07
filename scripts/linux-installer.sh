#!/bin/sh
# OwnNAS installer for Linux (x86_64 / aarch64, systemd).
# This header is followed by a tar.gz payload; scripts/package-installer.py builds the .run file.
#
#   sudo sh ownnas-VERSION-linux-ARCH.run                 # asks what it needs
#   sudo sh ownnas-VERSION-linux-ARCH.run --root /srv/media --user alice --yes
#   sudo sh ownnas-VERSION-linux-ARCH.run --uninstall
set -eu

VERSION="@VERSION@"
ARCH="@ARCH@"
PREFIX=/opt/ownnas
BIN=/usr/local/bin
UNIT=/etc/systemd/system/ownnas.service
CONF=/etc/ownnas/ownnas.env

# Not root: start again through sudo, with the same options.
if [ "$(id -u)" != 0 ] && [ "${1:-}" != --help ] && [ "${1:-}" != -h ]; then
	command -v sudo >/dev/null 2>&1 || { echo "Error: run it as root" >&2; exit 1; }
	echo "OwnNAS installs a system service, so it needs sudo."
	exec sudo sh "$0" "$@"
fi

ROOT=""; DATA=""; RUN_AS=""; ADDR=""; YES=0; NO_SERVICE=0; CLIENTS=1; ACCOUNT=""; UNINSTALL=0; HELPERS=""; RECONF=0

usage() {
	cat <<EOF
OwnNAS $VERSION installer ($ARCH)

Installs ownnas to $PREFIX, links it and the clients (ownnas-sync, ownnas-mount,
ownnascli) into $BIN, and runs the server as the systemd service "ownnas".
Running it again upgrades the binaries and keeps the settings in $CONF.

Options:
  --root DIR        folder to share (the library)
  --data DIR        OwnNAS's database and thumbnails (default /var/lib/ownnas)
  --user NAME       account the service runs as; it must be able to read and
                    write the library (default: the user who ran sudo)
  --addr HOST:PORT  address to listen on (default 0.0.0.0:8787)
  --account NAME    first OwnNAS sign-in name (the password is asked, or read
                    from OWNNAS_PASSWORD)
  --no-service      install the binaries only
  --no-clients      skip ownnas-sync, ownnas-mount and ownnascli
  --with-helpers    install the tools OwnNAS uses (video and HEIC thumbnails,
                    backups, text in PDFs and pictures) with apt, without asking
  --no-helpers      do not offer them
  --yes             take the defaults instead of asking
  --reconfigure     ask the questions again (current answers are the defaults)
  --uninstall       remove the service and binaries (the data and library stay)
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
		--root) ROOT=$2; shift 2 ;;
		--data) DATA=$2; shift 2 ;;
		--user) RUN_AS=$2; shift 2 ;;
		--addr) ADDR=$2; shift 2 ;;
		--account) ACCOUNT=$2; shift 2 ;;
		--no-service) NO_SERVICE=1; shift ;;
		--no-clients) CLIENTS=0; shift ;;
		--with-helpers) HELPERS=yes; shift ;;
		--no-helpers) HELPERS=no; shift ;;
		--yes|-y) YES=1; shift ;;
		--uninstall) UNINSTALL=1; shift ;;
		--reconfigure) RECONF=1; shift ;;
		-h|--help) usage; exit 0 ;;
		*) echo "Unknown option: $1 (see --help)" >&2; exit 2 ;;
	esac
done

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }
ask() { # ask VAR "Question" default
	if [ "$YES" = 1 ] || [ ! -t 0 ]; then eval "$1=\$3"; return; fi
	printf '%s [%s]: ' "$2" "$3"; read -r reply || reply=""
	[ -n "$reply" ] || reply=$3
	eval "$1=\$reply"
}

has_systemd() { command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; }

if [ "$UNINSTALL" = 1 ]; then
	if has_systemd && [ -f "$UNIT" ]; then
		systemctl disable --now ownnas 2>/dev/null || true
		rm -f "$UNIT"; systemctl daemon-reload
	fi
	for b in ownnas ownnas-sync ownnas-mount ownnascli; do
		[ -L "$BIN/$b" ] && rm -f "$BIN/$b"
	done
	rm -rf "$PREFIX"
	say "OwnNAS removed. Kept $CONF, the data folder and the library."
	exit 0
fi

# --- checks ---------------------------------------------------------------
machine=$(uname -m)
case "$ARCH:$machine" in
	x86_64:x86_64|x86_64:amd64|aarch64:aarch64|aarch64:arm64) ;;
	*) die "this installer is for $ARCH, but this computer is $machine" ;;
esac
glibc=$(ldd --version 2>/dev/null | head -n1 | grep -oE '[0-9]+\.[0-9]+$' || true)
if [ -n "$glibc" ]; then
	major=${glibc%.*}; minor=${glibc#*.}
	if [ "$major" -lt 2 ] || { [ "$major" = 2 ] && [ "$minor" -lt 38 ]; }; then
		die "OwnNAS needs glibc 2.38 or newer (Ubuntu 24.04, Debian 13, Fedora 39); this system has $glibc"
	fi
fi

# --- unpack -----------------------------------------------------------------
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
line=$(awk '/^__PAYLOAD_BELOW__$/ { print NR + 1; exit }' "$0")
tail -n +"$line" "$0" | tar xzf - -C "$work" || die "the installer is damaged; download it again"

# --- settings (an upgrade keeps the previous ones) ---------------------------
upgrade=0; S_ROOT=""; S_DATA=""; S_ADDR=""; S_USER=""
if [ -f "$CONF" ]; then
	upgrade=1
	# shellcheck disable=SC1090
	. "$CONF"
	S_ROOT=${OWNNAS_ROOT:-}; S_DATA=${OWNNAS_DATA:-}; S_ADDR=${OWNNAS_ADDR:-}
	S_USER=$(sed -n 's/^User=//p' "$UNIT" 2>/dev/null || true)
	if [ "$RECONF" = 0 ]; then
		[ -n "$ROOT" ] || ROOT=$S_ROOT
		[ -n "$DATA" ] || DATA=$S_DATA
		[ -n "$ADDR" ] || ADDR=$S_ADDR
		[ -n "$RUN_AS" ] || RUN_AS=$S_USER
	fi
fi

if [ "$NO_SERVICE" = 0 ]; then
	has_systemd || die "systemd is not running here; use --no-service and start ownnas yourself"
	default_user=${SUDO_USER:-}
	[ "$default_user" != root ] || default_user=""
	[ -n "$RUN_AS" ] || ask RUN_AS "Run OwnNAS as user" "${S_USER:-${default_user:-ownnas}}"
	[ -n "$ROOT" ] || ask ROOT "Folder to share" "${S_ROOT:-$(getent passwd "$RUN_AS" | cut -d: -f6)}"
	[ -n "$DATA" ] || ask DATA "OwnNAS data folder" "${S_DATA:-/var/lib/ownnas}"
	[ -n "$ADDR" ] || ask ADDR "Listen on" "${S_ADDR:-0.0.0.0:8787}"
	[ -n "$ROOT" ] || die "pass --root with the folder to share"
	ROOT=$(realpath -m "$ROOT"); DATA=$(realpath -m "$DATA")
	[ -d "$ROOT" ] || die "$ROOT does not exist"
	case "$DATA/" in "$ROOT"/*) die "keep the data folder outside the shared folder" ;; esac

	if ! id "$RUN_AS" >/dev/null 2>&1; then
		[ "$RUN_AS" = ownnas ] || die "there is no user $RUN_AS"
		useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin ownnas
		say "Created the system user ownnas. Give it access to $ROOT, for example: sudo chown -R ownnas $ROOT"
	fi
fi

# --- helper tools (Debian, Ubuntu) ---------------------------------------------
# ffmpeg: video thumbnails; libheif + its HEVC plugin: HEIC photos (Debian and Ubuntu ship the
# decoder apart, and heif-convert reads no iPhone photo without it); restic: backups;
# tesseract and poppler: words in pictures and PDFs for search; smartmontools: drive health.
HELPER_PACKAGES="ffmpeg libheif-examples libheif-plugin-libde265 restic tesseract-ocr poppler-utils smartmontools"
if command -v apt-get >/dev/null 2>&1 && [ "$HELPERS" != no ]; then
	if [ -z "$HELPERS" ]; then
		if [ "$YES" = 1 ] || [ ! -t 0 ]; then
			HELPERS=no
		else
			ask HELPERS "Install the tools OwnNAS uses ($HELPER_PACKAGES)? yes/no" "yes"
		fi
	fi
	case "$HELPERS" in
		y|Y|yes|YES|Yes)
			say "Installing helper tools…"
			apt-get update -qq || true
			# One at a time: a package missing on an older release must not stop the others.
			for package in $HELPER_PACKAGES; do
				DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "$package" >/dev/null 2>&1 || echo "  (could not install $package)"
			done ;;
	esac
fi

# --- binaries ---------------------------------------------------------------
if has_systemd && systemctl is-active --quiet ownnas 2>/dev/null; then
	say "Stopping ownnas…"
	systemctl stop ownnas
fi
mkdir -p "$PREFIX"
[ -f "$PREFIX/ownnas" ] && cp -p "$PREFIX/ownnas" "$PREFIX/ownnas.previous"
install -m 755 "$work/ownnas" "$PREFIX/ownnas"
ln -sf "$PREFIX/ownnas" "$BIN/ownnas"
if [ "$CLIENTS" = 1 ]; then
	for b in ownnas-sync ownnas-mount ownnascli; do
		[ -f "$work/$b" ] || continue
		install -m 755 "$work/$b" "$PREFIX/$b"
		ln -sf "$PREFIX/$b" "$BIN/$b"
	done
fi
"$PREFIX/ownnas" --version >/dev/null 2>&1 || die "the new binary does not start here: $("$PREFIX/ownnas" --version 2>&1 | head -n3)"

if [ "$NO_SERVICE" = 1 ]; then
	say "Installed OwnNAS $VERSION to $PREFIX. Start it with: ownnas serve --root <folder> --data <folder>"
	exit 0
fi

# The service user replaces the binary itself when it installs an update from Settings → Updates.
chown -R "$RUN_AS" "$PREFIX"
mkdir -p "$DATA" "$(dirname "$CONF")"
chown "$RUN_AS" "$DATA"
chmod 700 "$DATA"

# --- first account ------------------------------------------------------------
as_user() { runuser -u "$RUN_AS" -- "$@"; }
users=$(as_user "$PREFIX/ownnas" user list --data "$DATA" 2>/dev/null || true)
if [ -z "$users" ]; then
	if [ -z "$ACCOUNT" ]; then ask ACCOUNT "First OwnNAS account name" "admin"; fi
	pass=${OWNNAS_PASSWORD:-}
	if [ -z "$pass" ]; then
		[ -t 0 ] || die "no account yet: run again in a terminal, or set OWNNAS_PASSWORD"
		while :; do
			printf 'Password for %s: ' "$ACCOUNT"; stty -echo; read -r pass; stty echo; echo
			printf 'Again: '; stty -echo; read -r again; stty echo; echo
			[ "$pass" = "$again" ] && [ ${#pass} -ge 8 ] && break
			echo "They differ, or are shorter than 8 characters. Try again."
		done
	fi
	OWNNAS_PASSWORD=$pass as_user "$PREFIX/ownnas" user add --data "$DATA" --username "$ACCOUNT"
fi

# --- service ------------------------------------------------------------------
cat >"$CONF" <<EOF
# Settings for the ownnas service. Change them, then: sudo systemctl restart ownnas
OWNNAS_ROOT=$ROOT
OWNNAS_DATA=$DATA
OWNNAS_ADDR=$ADDR
EOF
cat >"$UNIT" <<EOF
[Unit]
Description=OwnNAS
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$RUN_AS
EnvironmentFile=$CONF
ExecStart=$PREFIX/ownnas serve --restart-by-supervisor
WorkingDirectory=$DATA
# After installing an update OwnNAS exits normally; systemd starts the new binary.
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now ownnas >/dev/null 2>&1 || systemctl enable --now ownnas

sleep 2
if systemctl is-active --quiet ownnas; then
	port=${ADDR##*:}
	host=$(hostname -f 2>/dev/null || hostname)
	[ "$upgrade" = 1 ] && verb="Upgraded" || verb="Installed"
	say "$verb OwnNAS $VERSION. Open http://$host:$port"
	echo "  Settings: $CONF   Logs: journalctl -u ownnas -f"
	[ -f "$PREFIX/ownnas.previous" ] && echo "  The previous binary is kept as $PREFIX/ownnas.previous"
else
	journalctl -u ownnas -n 20 --no-pager || true
	die "ownnas did not start; see the log above"
fi
exit 0
__PAYLOAD_BELOW__
