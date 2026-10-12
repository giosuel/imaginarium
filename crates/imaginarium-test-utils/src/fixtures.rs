// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deterministic fixture generators for the performance budgets (PLAN §10).
//!
//! The generators produce **plain data** (positions, sizes, text), not
//! document-model types, so they work before the model exists and stay
//! stable when it changes. Higher crates convert the data into their own
//! types: the renderer prototype draws [`ShapeSpec`]s, the model builds nodes
//! from them, the layout engine paginates [`ParagraphSpec`]s.
//!
//! Every fixture is fully determined by its fields. The named constants
//! (`LARGE`, `SMALL`, …) are the canonical sizes used in benchmarks and
//! budgets. **Don't change their values**: that would make results
//! incomparable to earlier runs. Add a new constant instead.

use crate::rng::Rng;

/// Kind of a generated board shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    /// Axis-aligned rectangle (before rotation).
    Rectangle,
    /// Ellipse inscribed in the shape's box.
    Ellipse,
    /// Straight line from the box's top-left to its bottom-right corner.
    Line,
    /// Text box. The label is in [`ShapeSpec::label`].
    Text,
}

/// One generated board shape, in board coordinates (y points down).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeSpec {
    /// Shape kind.
    pub kind: ShapeKind,
    /// Left edge of the box.
    pub x: f64,
    /// Top edge of the box.
    pub y: f64,
    /// Box width (> 0).
    pub width: f64,
    /// Box height (> 0).
    pub height: f64,
    /// Clockwise rotation around the box center, in degrees.
    pub rotation_deg: f64,
    /// Fill color as `0xRRGGBB`.
    pub fill_rgb: u32,
    /// Short text for [`ShapeKind::Text`] shapes, empty otherwise.
    pub label: String,
}

/// A deterministic board of shapes, e.g. for the "10,000 shapes at 60 fps"
/// budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardFixture {
    /// Number of shapes to generate.
    pub shapes: usize,
    /// Shapes are spread over a square `[0, extent)²` board area.
    pub extent: f64,
    /// RNG seed.
    pub seed: u64,
}

impl BoardFixture {
    /// The PLAN §10 budget board: 10,000 shapes.
    pub const LARGE: Self = Self {
        shapes: 10_000,
        extent: 20_000.0,
        seed: 0x00B0_A2D0_0001_0000,
    };

    /// A small board for fast unit tests.
    pub const SMALL: Self = Self {
        shapes: 100,
        extent: 2_000.0,
        seed: 0x00B0_A2D0_0000_0100,
    };

    /// Generates the shapes. Same fixture → same shapes, in the same order.
    pub fn shapes(&self) -> impl Iterator<Item = ShapeSpec> + use<> {
        let mut rng = Rng::new(self.seed);
        let extent = self.extent;
        (0..self.shapes).map(move |index| {
            let kind = *rng.pick(&[
                ShapeKind::Rectangle,
                ShapeKind::Rectangle,
                ShapeKind::Ellipse,
                ShapeKind::Line,
                ShapeKind::Text,
            ]);
            let width = rng.range_f64(20.0, 400.0);
            let height = rng.range_f64(20.0, 300.0);
            let rotation_deg = if rng.chance(0.2) {
                rng.range_f64(-45.0, 45.0)
            } else {
                0.0
            };
            let label = if kind == ShapeKind::Text {
                format!("{} {index}", rng.pick(WORDS))
            } else {
                String::new()
            };
            ShapeSpec {
                kind,
                x: rng.range_f64(0.0, extent),
                y: rng.range_f64(0.0, extent),
                width,
                height,
                rotation_deg,
                fill_rgb: (rng.next_u64() & 0x00FF_FFFF) as u32,
                label,
            }
        })
    }
}

/// Role of a generated paragraph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParagraphRole {
    /// Chapter heading.
    Heading1,
    /// Section heading.
    Heading2,
    /// Body text.
    Body,
    /// Bulleted list item.
    ListItem,
}

/// One generated paragraph of a document.
#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphSpec {
    /// Structural role, mapped to a paragraph style by the consumer.
    pub role: ParagraphRole,
    /// Plain text (words separated by single spaces, sentences end with `.`).
    pub text: String,
}

/// A deterministic text document, e.g. for the "open a 300-page document"
/// budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocumentFixture {
    /// Target length in pages, at [`DocumentFixture::WORDS_PER_PAGE`] words
    /// per page.
    pub pages: usize,
    /// RNG seed.
    pub seed: u64,
}

impl DocumentFixture {
    /// Average words per page used to size the document.
    pub const WORDS_PER_PAGE: usize = 450;

    /// The PLAN §10 budget document: 300 pages.
    pub const LARGE: Self = Self {
        pages: 300,
        seed: 0x00D0_C000_0300_0000,
    };

    /// A small document for fast unit tests.
    pub const SMALL: Self = Self {
        pages: 3,
        seed: 0x00D0_C000_0003_0000,
    };

    /// Generates paragraphs until the target word count is reached. A
    /// chapter heading starts every ~40 paragraphs, and sections and lists
    /// appear in between.
    pub fn paragraphs(&self) -> impl Iterator<Item = ParagraphSpec> + use<> {
        let mut rng = Rng::new(self.seed);
        let target_words = self.pages * Self::WORDS_PER_PAGE;
        let mut words_so_far = 0usize;
        let mut index = 0usize;
        std::iter::from_fn(move || {
            if words_so_far >= target_words {
                return None;
            }
            let (role, words) = if index.is_multiple_of(40) {
                (ParagraphRole::Heading1, 2 + rng.below(4) as usize)
            } else if index.is_multiple_of(8) {
                (ParagraphRole::Heading2, 3 + rng.below(5) as usize)
            } else if rng.chance(0.15) {
                (ParagraphRole::ListItem, 6 + rng.below(15) as usize)
            } else {
                (ParagraphRole::Body, 40 + rng.below(80) as usize)
            };
            index += 1;
            words_so_far += words;
            Some(ParagraphSpec {
                role,
                text: sentence_text(&mut rng, words),
            })
        })
    }
}

/// One generated slide.
#[derive(Clone, Debug, PartialEq)]
pub struct SlideSpec {
    /// Slide title.
    pub title: String,
    /// Bullet points (3–6).
    pub bullets: Vec<String>,
    /// Decorative shapes on the slide, positioned within a 1920×1080 frame.
    pub shapes: Vec<ShapeSpec>,
}

/// A deterministic presentation, e.g. for slide-sorter and thumbnail
/// benchmarks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckFixture {
    /// Number of slides.
    pub slides: usize,
    /// RNG seed.
    pub seed: u64,
}

impl DeckFixture {
    /// The canonical large deck: 100 slides.
    pub const LARGE: Self = Self {
        slides: 100,
        seed: 0x00DE_C000_0100_0000,
    };

    /// Generates the slides.
    pub fn slides(&self) -> impl Iterator<Item = SlideSpec> + use<> {
        let mut rng = Rng::new(self.seed);
        (0..self.slides).map(move |index| {
            let title = format!("{} {}", capitalized(rng.pick(WORDS)), index + 1);
            let bullets = (0..3 + rng.below(4))
                .map(|_| {
                    let words = 4 + rng.below(8) as usize;
                    sentence_text(&mut rng, words)
                })
                .collect();
            let board = BoardFixture {
                shapes: rng.below(6) as usize,
                extent: 1_000.0,
                seed: rng.next_u64(),
            };
            SlideSpec {
                title,
                bullets,
                shapes: board.shapes().collect(),
            }
        })
    }
}

/// Builds `words` words of sentence-like text: capitalized starts, full
/// stops every 6–18 words.
fn sentence_text(rng: &mut Rng, words: usize) -> String {
    let mut text = String::with_capacity(words * 7);
    let mut until_stop = 6 + rng.below(13) as usize;
    let mut start_of_sentence = true;
    for i in 0..words {
        if i > 0 {
            text.push(' ');
        }
        let word = rng.pick(WORDS);
        if start_of_sentence {
            text.push_str(&capitalized(word));
        } else {
            text.push_str(word);
        }
        until_stop -= 1;
        start_of_sentence = until_stop == 0 || i + 1 == words;
        if start_of_sentence {
            text.push('.');
            until_stop = 6 + rng.below(13) as usize;
        }
    }
    text
}

/// Uppercases the first character of an ASCII word.
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Fixed vocabulary for generated text. Never reorder or edit it: that
/// changes all fixture text. Append only.
const WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
    "ex",
    "ea",
    "commodo",
    "consequat",
    "duis",
    "aute",
    "irure",
    "in",
    "reprehenderit",
    "voluptate",
    "velit",
    "esse",
    "cillum",
    "fugiat",
    "nulla",
    "pariatur",
    "excepteur",
    "sint",
    "occaecat",
    "cupidatat",
    "non",
    "proident",
    "sunt",
    "culpa",
    "qui",
    "officia",
    "deserunt",
    "mollit",
    "anim",
    "id",
    "est",
    "laborum",
    "board",
    "shape",
    "diagram",
    "slide",
    "page",
    "layer",
    "canvas",
    "render",
    "imaginarium",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_is_deterministic_and_sized() {
        let a: Vec<_> = BoardFixture::SMALL.shapes().collect();
        let b: Vec<_> = BoardFixture::SMALL.shapes().collect();
        assert_eq!(a, b);
        assert_eq!(a.len(), 100);
        assert_eq!(BoardFixture::LARGE.shapes().count(), 10_000);
    }

    #[test]
    fn board_shapes_are_within_bounds() {
        let fixture = BoardFixture::SMALL;
        for shape in fixture.shapes() {
            assert!((0.0..fixture.extent).contains(&shape.x));
            assert!((0.0..fixture.extent).contains(&shape.y));
            assert!(shape.width > 0.0 && shape.height > 0.0);
            assert!(shape.fill_rgb <= 0xFF_FFFF);
            assert_eq!(shape.kind == ShapeKind::Text, !shape.label.is_empty());
        }
    }

    #[test]
    fn different_seeds_give_different_boards() {
        let other = BoardFixture {
            seed: 1,
            ..BoardFixture::SMALL
        };
        assert_ne!(BoardFixture::SMALL.shapes().next(), other.shapes().next());
    }

    #[test]
    fn document_reaches_its_target_length() {
        let fixture = DocumentFixture::LARGE;
        let paragraphs: Vec<_> = fixture.paragraphs().collect();
        let words: usize = paragraphs.iter().map(|p| p.text.split(' ').count()).sum();
        let target = fixture.pages * DocumentFixture::WORDS_PER_PAGE;
        assert!(words >= target, "{words} words < target {target}");
        assert!(words < target + 200, "overshoot: {words}");
        assert_eq!(paragraphs[0].role, ParagraphRole::Heading1);
        assert!(paragraphs.iter().any(|p| p.role == ParagraphRole::ListItem));
    }

    #[test]
    fn document_text_looks_like_sentences() {
        for paragraph in DocumentFixture::SMALL.paragraphs() {
            assert!(paragraph.text.ends_with('.'));
            assert!(paragraph.text.chars().next().unwrap().is_ascii_uppercase());
            assert!(!paragraph.text.contains("  "));
        }
    }

    #[test]
    fn deck_has_titles_bullets_and_is_deterministic() {
        let a: Vec<_> = DeckFixture::LARGE.slides().collect();
        assert_eq!(a.len(), 100);
        assert!(a.iter().all(|s| (3..=6).contains(&s.bullets.len())));
        assert_eq!(a, DeckFixture::LARGE.slides().collect::<Vec<_>>());
    }
}
