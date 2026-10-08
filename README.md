`rex` is a terminal music player for the music library on your Plex Media Server, built to stay small and fast!

## Installation

There are a few packages available of this program for various linux flavours, just select one from the releases list.

## Usage

If you want to specify configuration values yourself, have a look at the `config.example.toml`, edit the values, and move it to `~/.config/rex/config.toml` or `$XDG_CONFIG_HOME/rex/config.toml` and rex will read it on startup. `client_identifier` and `volume` values **must** be specified if you make your own `config.toml`.

```sh
rex                   # sign in on first run, then open the player
rex login             # sign in again and pick a server and music library
rex logout            # forget the saved tokens
rex --log-level debug # more detail in ~/.local/state/rex/rex.*.log
```

press `?` inside the player to see all keybindings
