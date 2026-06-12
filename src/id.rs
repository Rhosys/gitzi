use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use std::hash::{DefaultHasher, Hash, Hasher};
use uuid::Uuid;

/// 256-word list for deterministic slug generation.
const WORDS: &[&str; 256] = &[
    "ace", "arc", "ash", "axe", "bay", "bee", "bit", "bow",
    "box", "bud", "bus", "cap", "car", "cat", "cob", "cod",
    "cog", "cow", "cub", "cup", "cut", "dam", "dew", "dig",
    "dim", "dot", "dug", "dye", "ear", "eel", "egg", "elk",
    "elm", "emu", "eye", "fan", "far", "fig", "fin", "fir",
    "fit", "fix", "fly", "fog", "fox", "fur", "gap", "gas",
    "gem", "gin", "gum", "gut", "gym", "hat", "hay", "hen",
    "hex", "hip", "hog", "hop", "hot", "hub", "hue", "hum",
    "ice", "imp", "ink", "inn", "ion", "ire", "ivy", "jab",
    "jam", "jar", "jaw", "jay", "jet", "jig", "jog", "jot",
    "joy", "jug", "key", "kid", "kit", "lab", "lag", "lap",
    "law", "lay", "leg", "let", "lid", "lip", "lit", "log",
    "lot", "low", "lug", "map", "mat", "maw", "mix", "mob",
    "mod", "mop", "mud", "mug", "nap", "net", "nib", "nod",
    "nor", "not", "now", "nut", "oak", "oar", "oat", "odd",
    "ode", "oil", "old", "one", "orb", "ore", "our", "out",
    "owl", "own", "pad", "pan", "paw", "pea", "peg", "pen",
    "pet", "pie", "pig", "pin", "pit", "ply", "pod", "pop",
    "pot", "pub", "pug", "pun", "put", "rag", "ram", "ran",
    "rap", "rat", "raw", "ray", "red", "rib", "rid", "rig",
    "rim", "rip", "rod", "roe", "rot", "row", "rub", "rug",
    "rum", "run", "rut", "rye", "sap", "saw", "sea", "set",
    "shy", "sip", "sit", "six", "ski", "sky", "sly", "sob",
    "sod", "son", "sow", "spa", "spy", "sum", "sun", "tab",
    "tag", "tan", "tap", "tar", "tax", "tea", "ten", "the",
    "tie", "tin", "tip", "toe", "ton", "top", "tow", "toy",
    "try", "tub", "tug", "two", "urn", "van", "vat", "vet",
    "vim", "vow", "wag", "war", "wax", "way", "web", "wet",
    "wig", "win", "wit", "woe", "wok", "won", "yak", "yam",
    "yap", "yaw", "yew", "yip", "zap", "zen", "zig", "zip",
    "zoo", "arm", "art", "bag", "ban", "bar", "bat", "bed",
    "big", "bin", "bog", "bot", "bow", "bug", "bun", "but",
];

/// Generate a new resource ID: `{uuid7_base64url}-{three-word-slug}`.
///
/// The base64url prefix is 22 characters (16 UUID bytes encoded without padding).
/// The slug is derived deterministically from the title.
pub fn new_id(title: &str) -> String {
    let uuid = Uuid::now_v7();
    let b64 = URL_SAFE_NO_PAD.encode(uuid.as_bytes());
    let slug = derive_slug(title);
    format!("{b64}-{slug}")
}

/// Deterministic 3-word slug from title using a hash to index into WORDS.
///
/// Same title always produces the same slug. Uses `DefaultHasher` (SipHash)
/// and extracts 3 bytes from the hash to select words.
pub fn derive_slug(title: &str) -> String {
    let mut hasher = DefaultHasher::new();
    title.hash(&mut hasher);
    let hash = hasher.finish().to_le_bytes();

    // Each byte selects one word (256 words = perfect byte indexing)
    let w1 = WORDS[hash[0] as usize];
    let w2 = WORDS[hash[1] as usize];
    let w3 = WORDS[hash[2] as usize];

    format!("{w1}-{w2}-{w3}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_id_format_is_valid() {
        let id = new_id("test task");
        let parts: Vec<&str> = id.splitn(2, '-').collect();
        assert_eq!(parts[0].len(), 22, "base64url prefix must be 22 chars");

        // Decode the base64url prefix to verify it's 16 bytes
        let bytes = URL_SAFE_NO_PAD.decode(parts[0]).unwrap();
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn slug_is_deterministic() {
        let slug1 = derive_slug("my feature title");
        let slug2 = derive_slug("my feature title");
        assert_eq!(slug1, slug2);
    }

    #[test]
    fn slug_has_three_words() {
        let slug = derive_slug("anything");
        let words: Vec<&str> = slug.split('-').collect();
        assert_eq!(words.len(), 3);
        for w in &words {
            assert!(WORDS.contains(w), "word '{w}' not in WORDS list");
        }
    }

    #[test]
    fn different_titles_produce_different_slugs() {
        let s1 = derive_slug("add login feature");
        let s2 = derive_slug("fix payment bug");
        assert_ne!(s1, s2);
    }
}
