# Locus

Locus is a system monitor for Wayland desktops, written in Rust with GTK 4 and libadwaita.
It was made for the Meridian desktop and depends on nothing of it, so it runs on any other.

Processes lists what runs, with the share of the processor, the memory and the disk traffic
of each, sorted by any of them and searchable by name, command line or ID. A process can be
ended, killed, stopped and continued from its menu, a right click or a long press, or from
the bar beneath the list; ending and killing ask first. Resources graphs the processor as a
whole and each of its cores, memory and swap, the network and the disks, and shows the room
left on each mounted disk.

Everything comes from `/proc` and `/sys`, read on a worker thread. The processes are read
only while their list is on screen, since reading every one of them is most of what a
reading costs; the graphs keep going behind it. Readings come every 2 seconds, or every 1 or
5 as chosen in the main menu.

Locus signals processes as the user running it, so the processes of other users, root's
included, cannot be ended from it, and their disk traffic is not shown: the kernel keeps
both to their owner.

## Building and installing

    just build
    sudo just install                   # prefix /usr/local
    just prefix=$HOME/.local build install

Build needs Rust 1.92, `blueprint-compiler`, `just`, gettext and the development packages
for GTK 4.22 and libadwaita 1.8 or newer. `just run` starts the debug build without
installing it. `just uninstall` removes what `install` put in place, with the same prefix.

The interface is available in English, Russian and Ukrainian.

- [Contributing](CONTRIBUTING.md), including where things are in the code, and
  [reporting a vulnerability](SECURITY.md)

GPL-3.0-or-later.
