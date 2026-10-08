`rex` is a terminal music player for the music library on your Plex Media Server, built to stay small and fast!

## Installation

There are a few packages available of this program for various linux flavours, just select one from the releases list.

## Usage

```sh
rex                   # sign in on first run, then open the player
rex login             # sign in again and pick a server and music library
rex logout            # forget the saved tokens
rex --log-level debug # more detail in ~/.local/state/rex/rex.*.log
```

press `?` inside the player to see all keybindings

## Configuration

rex keeps its settings in `~/.config/rex/config.toml` (or `$XDG_CONFIG_HOME/rex/config.toml`). It creates the file on first run and `rex login` fills in the server details, so you only need to edit it to change things like `accent_color`. Every key is optional, and anything you leave out gets its default. `config.example.toml` describes each key, but don't copy it over as-is, since its tokens and server values are placeholders.

**Edit the file while rex isn't running!** rex writes the volume and server address back to it on exit, which overwrites changes made in the meantime and drops any comments.
