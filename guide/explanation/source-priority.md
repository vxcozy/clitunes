# Source priority: Connect vs local verbs

When a Spotify Connect client is driving playback and the user (or a
script) issues a local verb like `clitunes source radio <uuid>`, which
one wins?

## Decision

**Local verbs always win.** A `PlayRadio`, `PlaySpotify`, `PlayLocal`,
or `PlayTone` source command interrupts the Connect source immediately.
The daemon stops pumping PCM from the Spirc-controlled sink and switches
to the new source.

Connect does not fight back. The Spirc task stays alive (the phone still
shows the device as "connected"), but no audio flows until the Spirc is
either disconnected or the phone re-picks the device after the local
source finishes. In practice the Spirc task usually ends on its own
shortly after the PCM drain stops, because Spotify's backend detects the
idle device.

## Why

1. **User agency.** clitunes is a tool the user controls from their
   terminal. An external Connect client should never lock the user out
   of their own player. If someone sitting at the keyboard types
   `clitunes source radio ...`, that intent is unambiguous and must take
   effect.

2. **No hidden state fights.** If Connect could override local verbs,
   the user would see their command succeed (`ok: true`) but hear the
   wrong audio — a confusing failure mode with no obvious cause. The
   rule "local always wins" is simple enough to explain in one sentence.

3. **Reconnection is cheap.** If the Connect user wants playback back,
   they re-pick the device from their phone. Discovery keeps advertising
   regardless of source-pipeline state, so the device stays visible.

## The other direction

When Connect credentials arrive (Discovery yields a Zeroconf handshake)
while a local source is playing, the daemon does switch to Connect.
This is intentional: the phone user explicitly chose this device, so the
daemon honours that intent the same way a real Spotify speaker would.

The distinction: **inbound Connect credentials are an explicit user
action** (someone tapped the device in the Spotify app). A local verb is
also an explicit user action. Both win at the moment they fire. The last
action wins — no priority hierarchy, just temporal ordering.

## Disconnect

`clitunes connect disconnect` tears down the Spirc but keeps Discovery
running. After a disconnect, the source pipeline returns to idle. The
device stays visible in the Spotify app for re-picking.
