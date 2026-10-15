`rex` is a terminal music player for the music library on your Plex Media Server, built to stay small and fast!

<img src="./images/one.png" alt="image of player maximized"/>
<img src="./images/two.png" alt="image of player minimized"/>

## Installation

There are a few packages available of this program for various linux flavours, just select one from the releases list.

### Building from source

```sh
cargo install --path .                              # local playback only
cargo install --path . --features cast              # + Chromecast / Google Cast, links the system OpenSSL
cargo install --path . --features vendored-openssl  # + cast, with OpenSSL built in (needs perl, make and a C compiler)
```

Building needs the ALSA headers (`libasound2-dev` / `alsa-lib`) and `pkg-config`; `cast` also needs the OpenSSL headers (`libssl-dev` / `openssl`). Packagers can map `cast` to an optional feature, such as a `cast` USE flag.

Try a cast device with `rex debug play <rating-key> --cast <device-ip>`. The device fetches the audio straight from your Plex server, so the server address must be reachable from it.

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
