# Colouring Book Studio

Turn your own story and characters into a printable **read-and-colour book** for children aged 5–12.
You paste or upload the story, describe the characters, and the app splits the story into pages,
draws a colouring picture for each page using **free** AI, cleans every picture into crisp
print-ready line art, and exports a PDF. Children can also read and colour the book on a tablet or
computer: tap a space to fill it, with an optional read-aloud button.

Written in Rust (web server, image clean-up, PDF) with a browser interface. Everything runs on
your own computer, and your books stay in the `library/` folder.

## Quick start

1. Install Rust: <https://rustup.rs>
2. In this folder run:
   ```sh
   cargo run --release
   ```
3. Open <http://127.0.0.1:8080> in your browser.

Then work through the tabs from left to right: **Book → Characters → Story → Pictures → Read & Colour**.
Press **Download printable PDF** on the Book tab when you're done.

To colour on a tablet on the same Wi-Fi, start with `HOST=0.0.0.0 cargo run --release` and open
`http://<your-computer's-IP>:8080` on the tablet. Only do this on a network you trust: the app has no login.

| Setting | Default | Meaning |
|---|---|---|
| `PORT` | `8080` | Port to listen on |
| `HOST` | `127.0.0.1` | Interface to listen on |
| `CBS_DATA_DIR` | `library` | Where books and settings are saved |
| `HF_TOKEN` | – | Hugging Face token (can also be entered in the app) |
| `POLLINATIONS_TOKEN` | – | Optional Pollinations token |
| `RUNWARE_API_KEY` | – | Runware API key (can also be entered in the app) |

## AI options (⚙ AI settings tab)

| Option | Cost | Quality | Notes |
|---|---|---|---|
| **Runware** (recommended) | Pay-as-you-go, about US$0.01 per picture, roughly **US$0.30–0.50 per 30-page book** | Very good (FLUX), and **keeps characters consistent** | Uses character reference pictures (below). Needs an API key from runware.ai. |
| **Pollinations.ai** | Free, no account | Good (FLUX) | Rate-limited. No reference pictures, so characters vary from page to page. |
| **Hugging Face** | Free account token | Good (FLUX.1-schnell) | The small monthly free allowance runs out quickly. |
| **Your own Stable Diffusion** (AUTOMATIC1111 / Forge, `--api`) | Free, unlimited | Best once set up | Needs a decent NVIDIA GPU (≈ 6 GB+). |
| **Test drawings** | Free, offline | Not AI | For trying the app and checking layout. |

### Consistent characters with Runware

1. On the **Characters** tab, press **Make reference picture** for each character, or **Upload a drawing**
   of them (your own art works well). *Generate all missing pictures* does this first automatically.
2. Every page that mentions a character sends their reference picture (up to 3 characters, side by side)
   to FLUX.1 Kontext [dev] (`runware:106@1`, about $0.01), which redraws the same character in the new scene.
3. Pages without characters use FLUX.1 [dev] (`runware:101@1`). Both model ids can be changed in settings.

Set the key in the app, or start with `RUNWARE_API_KEY=... cargo run --release`.
Runware's prices change, so check the cost shown in its dashboard after the first few pictures.

Free services change their rules without notice. If one stops working, the error message is shown
in the app; switch to another provider in settings.

## What makes the pictures good colouring pages

Raw AI images often contain grey shading, speckles, solid black hair or shadows, and small gaps in
outlines that make colouring (and tap-to-fill) leak. Every picture, including drawings you upload,
goes through a clean-up step (`src/lineart.rs`):

1. Converted to pure black and white. Light grey shading becomes paper and dark grey becomes ink.
2. Specks and dust removed.
3. Small gaps in outlines closed, so each area is a sealed shape.
4. Large solid black areas hollowed out into thick outlines, so they can be coloured.
5. Lines thickened to suit the age group (thickest for ages 5–7).
6. Tiny leftover pockets that are too small to colour are filled in.

The age group also sets the words per page (about 50 / 100 / 160), the font size, and how detailed
the AI is asked to make each picture.

## Tips for the best results

- **Characters**: describe each one precisely and identically: animal or person, body shape, hair,
  clothes, and one or two stand-out features. That description is added to every picture they appear
  in. Add the other names the story uses for them (e.g. "Gran, Ouma") so they're recognised on each
  page. Free AI models can't guarantee identical characters on every page; precise descriptions
  get you most of the way, and **Try again** on any page that doesn't match.
- **Picture idea**: when a page's text covers several events, write one clear moment in the
  page's *Picture idea* box. Short, concrete scenes give the best pictures.
- **Page breaks**: put a line containing only `---` in the story wherever you want a new page.
  Otherwise the story is split automatically, or into the number of pages you choose. There's no
  page limit.
- **Picture prompts in the story file**: inside a page, a line starting with `Picture:` (continuing until the
  next blank line) becomes that page's picture idea and is not printed. A whole book can be imported this way.
- **Style picture**: on the Book tab, upload one approved picture as the *style reference*. With Runware, every
  new picture, including the characters' reference pictures, copies its drawing style.
- **Two editions, one set of art**: *Copy book with pictures* makes a copy you can rewrite for another age group.
- **KDP-style interior**: layout *Square picture, text below*, paper *US Letter*, *Blank back after each picture*,
  spelling *American*. Pictures are processed at 3072 px, which is above 300 DPI at that size.
- **Your own art**: *Upload a drawing* accepts a photo or scan of a hand-drawn sketch and cleans it
  up the same way.
- **Printing**: the default layout puts the story text on one page and a full-page picture on the
  next. Print **single-sided** so markers don't bleed through onto the story.

## Development

```sh
cargo test          # story splitting, prompts, line-art clean-up, PDF structure
cargo run --release
```

| File | Purpose |
|---|---|
| `src/main.rs` | HTTP API and server |
| `src/story.rs` | Page splitting, character detection, prompt building |
| `src/ai.rs` | Image providers |
| `src/lineart.rs` | Clean-up into colouring-book line art |
| `src/pdf.rs` | Built-in PDF writer (1-bit images, so books stay small) |
| `src/store.rs` | Saving books to disk |
| `static/index.html` | The whole browser interface (embedded in the binary) |
