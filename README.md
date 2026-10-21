`rexamp` is a terminal music player for the music library on your Plex Media Server, built to stay small and fast!

<img src="./images/one.png" alt="image of player maximized"/>
<img src="./images/two.png" alt="image of player minimized"/>

## Installation

There are a few packages available of this program for various linux flavours, just select one from the releases list.

If you are on macOS you can install it using homebrew: `brew install defaultdino/tap/rex`

### Building from source

```sh
cargo install --path .                              # local playback only
```

## Usage

```sh
rexamp                   # sign in on first run, then open the player
rexamp login             # sign in again and pick a server and music library
rexamp switch            # switch music libraries/sections
rexamp logout            # forget the saved tokens
rexamp --log-level debug # more detail in ~/.local/state/rexamp/rexamp.*.log
```

press `?` inside the player to see all keybindings

## Configuration

rexamp reads its settings from `~/.config/rexamp/config.toml` (or `$XDG_CONFIG_HOME/rexamp/config.toml`). Every key is optional, and `config.example.toml` describes each one.

All rexamp state can be found in `~/.local/state/rexamp/state.json`. Coming from 1.5 or earlier, rexamp copies the login and server keys from `config.toml` there on first start; after that you can delete them from `config.toml`.
