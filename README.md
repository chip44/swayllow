# swayllow

Window swallowing for [Sway](https://swaywm.org/), inspired by the [dwm swallow patch](https://dwm.suckless.org/patches/swallow/).

Only terminals (specified with `--terminal` or `-t`) can swallow other windows.

## Usage

This program is intended to be run in the background.
To start it automatically, put it in your Sway config like this:

```sh
exec --no-startup-id swayllow \
  --terminal 'app_id=foot' \
  --noswallow 'app_id=wev'
```

For more information run `swayllow --help`.
