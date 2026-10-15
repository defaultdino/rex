`rex` is a terminal music player for the music library on your Plex Media Server, built to stay small and fast!

<img src="./images/one.png" alt="image of player maximized"/>
<img src="./images/two.png" alt="image of player minimized"/>

## Installation

There are a few packages available of this program for various linux flavours, just select one from the releases list.

### Building from source

```sh
cargo install --path .                              # local playback only
```

## Usage

```sh
rex                   # sign in on first run, then open the player
rex login             # sign in again and pick a server and music library
rex switch            # switch music libraries/sections
rex logout            # forget the saved tokens
rex --log-level debug # more detail in ~/.local/state/rex/rex.*.log
```

press `?` inside the player to see all keybindings

## Configuration

rex reads its settings from `~/.config/rex/config.toml` (or `$XDG_CONFIG_HOME/rex/config.toml`). Every key is optional, and `config.example.toml` describes each one.

All rex state can be found in `~/.local/state/rex/state.json`. Coming from 1.5 or earlier, rex copies the login and server keys from `config.toml` there on first start; after that you can delete them from `config.toml`.
