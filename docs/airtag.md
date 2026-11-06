# AirTags and Bluetooth addresses

Notes on how AirTags (and other Apple Find My accessories) show up to a bluey scanner, what is
known about their Bluetooth addresses, and what was observed while a tag had its battery removed
and replaced. The observations are from one tag next to one scanner; the research is secondary
except where marked.

## Summary

- A Find My accessory's Bluetooth address is **not a reliable identifier**. Sources agree it rotates,
  and in our observations it changed once between two battery removals.
- The address is derived from the tag's key pair, so only its owner can predict it. Outside observers
  can't. It changes about every 15 minutes while the owner's phone is nearby, and about once a day
  otherwise.
- Matching tags exactly needs their keys, which can only be extracted on a Mac. Without one, bluey
  can follow a labelled tag across rotations (see [Recognising your own tags](#recognising-your-own-tags)).
- What did work: the signal strength, and **when the tag stops advertising**. A tag with its battery
  out goes silent immediately and drops off the scanner's table about two minutes later.
- Taking the battery out and putting it back in does not appear to unpair the tag. Only a factory
  reset does (five presses of the battery cycle, per Apple's reset guide).

## Research

### Apple (official)

- [Replace the battery in your AirTag](https://support.apple.com/en-asia/102600): the battery
  procedure and the low-battery notification. It doesn't say anything about Bluetooth addresses.
- [How to reset your AirTag](https://support.apple.com/ms-my/102577): a tag is tied to one Apple
  Account. If the previous owner removed it while out of range, it must be reset before it can be used
  again. A factory reset is the step that erases pairing.

Apple's Find My accessory specification was not in the search results, so nothing below about address
rotation is confirmed by Apple.

### User reports

- Apple Community answers say a battery replacement doesn't need re-pairing and doesn't unpair the
  tag; only a factory reset does.
  ([Apple Community: AirTag won't activate after battery change](https://discussions.apple.com/thread/254661866))
- A third-party guide ([Apple Headlines](https://www.appleheadlines.com/how-to-change-airtag-battery/))
  repeats the same battery and pairing advice.

These are not official and were not tested against a specific tag.

### Research and reverse engineering (secondary)

- [Adam Catley's AirTag write-up](https://adamcatley.com/AirTag.html) reports that the broadcast
  identity changes about once a day, around 04:00 local time, when the tag is away from its owner. It
  also says a power cycle can trigger the change within 24 hours. The page says only the last byte of
  the advertisement changes in between. This is an independent analysis, not an Apple specification.
- [USENIX Security 2025 paper on Find My](https://www.usenix.org/system/files/conference/usenixsecurity25/sec25cycle1-prepub-1266-chen-junming.pdf)
  says the Find My specification calls for a random static address when advertising lost-device
  messages. It also reports that the implementation accepts many address types (public, NRPA, RPA and
  random static) and calls that a vulnerability. Random static addresses are usually fixed, so the
  rotation is a Find My design choice on top of the usual rule.

The rotating address is derived from the tag's rotating public key. Someone who has the key (the
owner's device) can predict the next address. Someone without it can't.

### The two advertising states

Find My accessories advertise in one of two states. The timings are from reverse engineering and
academic papers, not from Apple.

| State | When | What the advert carries | Key and address change |
|---|---|---|---|
| **Nearby** | Recently in contact with the owner's iPhone, so usually while you're home | A short Apple record (type `0x12`). Part of the current key is in the address. | About every **15 minutes** |
| **Separated** | Out of contact with the owner's devices for a while | A long Apple record (type `0x12`, about 25 bytes) with most of the public key. The rest is in the address. | About once a **day** (around 04:00), and after a power cycle |

The keys come from a master secret through a fixed key derivation. Anyone holding that secret can
work out every future key, and so every address. The master secret is only stored in the owner's
iCloud keychain.

- [AirGuard issue #47](https://github.com/seemoo-lab/airguard/issues/47): the AirGuard developers
  (who make an Android tracker detector) say stationary AirTags rotate their address every 15
  minutes, and that the status byte is not reliable enough to tell tags apart.
- [Turner & Bellon slides (2022)](https://allison-turner.github.io/media/AirTag%20Presentation%20-%20Allison%20Turner%20&%20Alex%20Bellon%203-14-2022.pdf)
  and [ar5iv 2202.11813](https://ar5iv.labs.arxiv.org/html/2202.11813): daily key rotation when
  separated, and the last byte changing more often.
- [arXiv 2604.10138](https://arxiv.org/pdf/2604.10138) and the
  [Help Net Security summary](https://helpnetsecurity.com/2026/04/17/apple-airtag-relay-attack-location)
  (April 2026): rotation once every 24 hours. Pulling the battery pauses rotation.

## Recognising your own tags

The goal: log when each of our AirTags is in range of a scanner at home. We only need "in range",
not distance or direction.

### With the tag's keys (the only exact method)

[FindMy.py](https://github.com/malmeloo/FindMy.py) (v0.6 and later) supports real AirTags and runs
on Linux. Its scanner decodes both states (`NearbyOfflineFindingDevice`,
`SeparatedOfflineFindingDevice`). `is_from(accessory)` says whether an advert came from a tag whose
keys you hold ([scanner API](https://docs.mikealmel.ooo/FindMy.py/reference/findmy/scanner/scanner/index.html)).

The hard part is getting the keys:

- They have to be extracted on a Mac signed into the owning Apple Account. The tag records under
  `~/Library/com.apple.icloud.searchpartyd/` are encrypted with a keychain item named `BeaconStore`
  ([henrik242 gist](https://gist.github.com/henrik242/1da3a252ca66fb7d17bca5509a67937f?permalink_comment_id=5587919),
  [YeapGuy gist](https://gist.github.com/YeapGuy/f473de53c2a4e8978bc63217359ca1e4?permalink_comment_id=4893415)).
- A February 2025 comment reports that newer macOS lets only Apple's Find My processes read
  `BeaconStore`. That would block the scripts.
- The FindMy.py maintainer confirms the keys must come from a Mac. They suggest a temporary macOS VM
  ([OSX-KVM](https://github.com/kholia/OSX-KVM)) for the one-time extraction, since the master key
  covers every future address
  ([FindMy.py #169](https://github.com/malmeloo/FindMy.py/issues/169)). Nobody in that thread
  reported a VM working. iCloud often refuses VMs that don't present believable Mac hardware, and
  Apple's licence allows macOS VMs only on Apple hardware.
- A Mac-free route through iCloud keychain sync is possible in principle but hasn't been written.

**We don't have a Mac**, so this is parked. If keys turn up later, the plan is: the scanner sends
raw advert data, and the host matches it with FindMy.py's logic. The firmware stays a plain
scanner. If the scanner ever needs to work alone, the host can work out the next day's addresses
(about 96 per tag) and send them over. The chip then only compares 6 bytes, with no P-224 maths on
it.

### Without keys: following a tag across rotations

You label a tag once ("the one at -40 dBm next to the scanner"), and bluey follows it across each
rotation. An address that goes silent and a new Find My address that appears within seconds, at a
similar signal strength, are treated as the same tag.

- This works best in the **separated** state. That is also the state that matters when the phone
  isn't home. The key then holds for about a day, and rotations happen at a predictable time.
- In the **nearby** state (phone at home) the tag rotates every 15 minutes, so following it is less
  reliable. But then the phone's own presence already answers most automation questions.
- It loses a tag that moves or loses power during a rotation, and it can swap two tags that rotate
  at the same moment with similar signal. A lost tag has to be labelled again.
- It needs the advert data, not just the address. See [Recording advert data](#recording-advert-data).

### Other routes

- **Apple's cloud reports.** FindMy.py, or the Home Assistant integration "FindMy Fleet"
  ([forum post](https://community.home-assistant.io/t/i-made-a-working-airtag-findmy-tracker/1013940)),
  fetch the location reports that other Apple devices upload. These need an Apple Account login and
  the tag's keys, give a rough position rather than "in range of this scanner", and don't use our
  scanners at all.
- **ESPresense, Bermuda and similar.** These have the same rotating-address problem
  ([HA forum](https://community.home-assistant.io/t/bluetooth-mac-vs-id-in-espresence/394580),
  [BLE tracker thread](https://community.home-assistant.io/t/tracking-proximity-of-bluetooth-le-trackers-airtag-chipolo-android-find-my/574894)).
  Home Assistant's "Private BLE Device" resolves iPhones and Watches with an IRK, but AirTags don't
  use IRK-resolvable addresses.
- **A beacon with a fixed ID** (iBeacon or Eddystone tag, or an nRF52 board we flash), or a DIY Find
  My tag whose keys we generate ourselves. This is reliable for items where "is it home?" matters.
  The AirTag can stay on them for finding them away from home.

## Recording advert data

The scanner sends each device's latest advertising data (not its scan response) in the snapshot's
`adv` field, from protocol version 4. `bluey log` writes it as hex in the JSON, and the
GeoPackage keeps it in `observations.adv` as a BLOB. For a Find My tag, that is enough to see the
state (short or long `0x12` record), the status byte, and the key bytes in each snapshot. That
should answer the open questions below and give the rotation-following code something to work
with.

First look, 2026-11-06 ~18:00 UTC, after flashing:

- `C5:87:F4:A9:96:A8` (the battery-pulled tag, -49 dBm) sends **two** kinds of advert:
  - the long separated-state Find My record: `1eff4c00 1219 20 1d0172443661a3debbc0b1dc4719b17f9b93e3a5dc9e02dd`
  - a service-data advert for UUID `0xFCB2`: `1e16b2fc 010010 1d0172443661…9e02dd`

  `0xFCB2` is very likely the cross-platform unwanted-tracker alert (DULT) format. Both carry the
  same key bytes, so keeping only the latest advert loses nothing needed for identification. The
  tag is in the separated state even though it's at home, perhaps because the battery pull broke
  its contact with the phone.
- Nine other Find My devices send the short nearby record, `07ff4c00 1202 SS HH`: a status byte and
  one more byte. Nearby adverts carry no key, so for these the address is all there is to go on.

## Observations on one tag

Times are UTC. Local time (EST) is 5 hours behind.

| Time (UTC) | Event |
|---|---|
| ~17:25–17:30 | `FA:E1:2C:11:AD:33` is the strongest BLE tag next to the scanner, -40 dBm (first seen at -40 in the 17:25 records; earlier records are not checked). |
| 17:29:59 | Three other tags (`D0:23:DE:E4:66:03`, `FE:FD:84:78:63:DA`, `E0:D5:1B:DC:9C:9F`) stop advertising. They were weaker and were probably moved out of range. |
| 17:30:27 | `FA:E1:2C:11:AD:33` stops advertising. The battery was removed at about this time (reported by the user). |
| 17:33:36 | `C5:87:F4:A9:96:A8` appears at -46 dBm, advertising about once a second. This is a new address near the same spot. |
| 17:36:08 | `C5:87:F4:A9:96:A8` stops advertising. The user had said the battery was in at 17:37 UTC, which doesn't match. The battery may have been out earlier than reported. |
| 17:38:26 | `C5:87:F4:A9:96:A8` is no longer in the snapshot. The battery was reported out at this point. |
| 17:39:29 | `C5:87:F4:A9:96:A8` is back at -43 dBm with the **same address**, after the battery went back in. |

What this shows:
- The first battery swap coincided with the address changing (`FA:E1:…` → `C5:87:…`). The second swap
  didn't change it. So the address changes occasionally, not on every swap. The first change could also
  be a different tag, but the timing and signal strength point to the same one.
- Both removals made the tag go silent right away, so the silence is a reliable sign of battery removal.
- The scanner keeps a device about two minutes after its last advert, so a dropped tag can stay listed
  for a while (`FA:E1:…` was still listed at -93 dBm after it went silent).

## How bluey shows this

- `bluey monitor`, `bluey snapshot` and `bluey tui` show the tag's current address and signal. The
  address shows as `static`, which is what Find My uses for AirTags.
- `bluey poll` and `bluey log` record each snapshot to the GeoPackage, so the timeline above can be
  rebuilt from `observations.last_seen`.
- The scanner only holds one device per address, so a rotated address looks like a new device.

## Open questions

- Does the address change only on a power cycle, a factory reset, or after about a day? The
  reverse-engineering source says a power cycle can trigger it, but that wasn't tested here.
- Are the adverts' last byte and the rest of the payload stable enough to link sightings? The
  scanner now records the advert data (`observations.adv`), so this can be checked.
- Which state are our tags in while the phone is home, and how long after the phone leaves do they
  switch to separated?
- Whether the tag's "last advert" timestamps can be used to tell apart two tags with different
  rotation schedules.
