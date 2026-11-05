//! Printer models, as Drivers report them and as slicers write them into
//! Print Files (PRD §7.3). A Print File is only ever sent to a Printer of the
//! model it was sliced for.
//!
//! The two sides name models differently: a Bambu Driver says `X1C`, Bambu
//! Studio writes `Bambu Lab X1 Carbon` or its code `BL-P001`. Both are
//! brought to one canonical form before comparing.

/// The canonical form of a model name: lower case, without the maker's name,
/// spaces or punctuation, with known aliases resolved.
pub fn canonical(model: &str) -> String {
    let mut name: String = model
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    for maker in ["bambulab", "originalprusa", "prusa"] {
        if let Some(rest) = name.strip_prefix(maker)
            && !rest.is_empty()
        {
            name = rest.to_owned();
        }
    }
    let alias = match name.as_str() {
        // Bambu Studio's model codes and long names.
        "blp001" | "x1carbon" => "x1c",
        "blp002" => "x1",
        "c13" => "x1e",
        "c11" => "p1p",
        "c12" => "p1s",
        "n1" => "a1mini",
        "n2s" => "a1",
        _ => return name,
    };
    alias.to_owned()
}

/// Whether two model names mean the same model.
pub fn same_model(a: &str, b: &str) -> bool {
    let (a, b) = (canonical(a), canonical(b));
    !a.is_empty() && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicer_and_driver_names_for_one_model_match() {
        assert!(same_model("X1C", "Bambu Lab X1 Carbon"));
        assert!(same_model("x1c", "BL-P001"));
        assert!(same_model("A1 mini", "Bambu Lab A1 mini"));
        assert!(same_model("A1 mini", "N1"));
        assert!(same_model("P1S", "C12"));
        assert!(same_model("MK4", "Original Prusa MK4"));
        assert!(same_model("Simulated", "simulated"));
    }

    #[test]
    fn different_models_do_not() {
        assert!(!same_model("X1C", "P1S"));
        assert!(!same_model("A1", "A1 mini"));
        assert!(!same_model("", ""));
        assert!(!same_model("X1", "X1C"));
    }
}
