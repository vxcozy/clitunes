# Customise the station picker

The built-in picker shows 12 genre slots. You can override any slot with a
specific station from [radio-browser.info](https://www.radio-browser.info/).

## Find a station UUID

Browse [radio-browser.info](https://www.radio-browser.info/), find a station
you like, and copy its UUID from the station detail page.

## Create an override file

Create `~/.config/clitunes/stations.toml`:

```toml
[[stations]]
slot = 1
name = "SomaFM Drone Zone"
url = "radiobrowser:a1234567-89ab-cdef-0123-456789abcdef"

[[stations]]
slot = 4
name = "KEXP Seattle"
url = "radiobrowser:b2345678-9abc-def0-1234-567890abcdef"
```

Slots you don't override keep their default genre labels. The picker shows
your custom station names in place of the genre placeholders.

## Reload

Restart `clitunes` to pick up changes. The daemon reloads the station list
on each client connection.
