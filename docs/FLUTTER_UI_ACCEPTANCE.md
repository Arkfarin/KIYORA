# Flutter UI acceptance — IN PROGRESS

The target is the Spotify desktop/Android composition and interaction model,
with KIYORA identity. No pixel-perfect or visual acceptance claim has been made.
There are no accepted Flutter screenshots yet. The React screenshots in
`docs/ux-overhaul` are historical feature context, not Flutter evidence.

## Visual source of truth

KIYORA palette from `src/styles/app.css`: background `#111214`, surface `#1c1d20`,
elevated `#25262a`, primary text `#f2f2ef`, secondary `#a5a6ab`, accent `#ff2448`.
Light palette: `#f4f4f2`, white surfaces, `#151517` text. Preserve bundled font
licensing (Manrope and Space Grotesk); never import Spotify brand assets.

The user explicitly selected Spotify composition; generic app-screen design
rules must not override the required artwork gradients, large collection titles,
three-part bottom player, card collections or mobile bottom navigation.

## Outstanding acceptance matrix

| Area | Required result | Current evidence |
| --- | --- | --- |
| Desktop shell | Top navigation, resizable library, independent content scroll, right context panel, persistent bottom player | Not implemented |
| Android shell | Home/Search/Library bottom navigation, mini player above navigation, native safe areas | Not implemented |
| Home | Real quick links, listening history, additions, albums/artists/favorites; no invented recommendations | Not implemented |
| Search/library | Typed paged Rust reads, all entity types, filters/sort, >500 results reachable | Rust track paging/search test exists; Flutter not implemented |
| Collection details | Playlist/album/artist headers, artwork, metadata, track lists and actual actions | Not implemented |
| Player | Real Rust transport, seek, shuffle/repeat, responsive full player, mini/full transitions and gestures | Typed bridge implemented; Flutter not implemented |
| Queue | Rust IDs/revisions, drag reorder and accessible alternatives, next/append/remove | Typed bridge implemented; Flutter not implemented |
| Lyrics | LRC/SRT/WebVTT/plain text, active line, follow/manual scroll, timestamp seek, transport visible | Existing Rust parser reused through typed read; Flutter not implemented |
| Management | Playlists, bulk selection, favorites, metadata/artwork/lyrics editing | Not implemented in Flutter |
| Import/export | Native pick/drop, original/edit flows, per-file progress, retry/cancel/duplicates, backups | Not implemented in Flutter |
| Onboarding/settings | Real auth, recovery/creation, account/themes/storage/sync/advanced | Typed auth/profile boundary implemented; Flutter not implemented |
| States/accessibility | Loading/empty/error/retry, keyboard/screen reader, reduced motion, scales and localization | Not implemented in Flutter |
| Performance | Separate progress subscriptions, lazy pages/artwork, 10k library and frame/memory measurements | 10,001-track Rust pagination tested; no Flutter performance evidence |

Acceptance requires screenshots of the running Flutter application at desktop,
tablet and phone sizes, comparisons against available Spotify references, plus
widget/navigation/golden/integration/accessibility tests. Test images/data must
be original or appropriately licensed and remain outside production data paths.
