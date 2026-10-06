//! Built-in help (T-438): short French pages compiled into the binary, so
//! the help opens without any network (a régie often has none).
//!
//! Each page is a plain HTML fragment of our own (headings, paragraphs,
//! lists, `<kbd>`, inline SVG drawn by us): no script, no external link or
//! resource. Links between pages are internal (`#aide/<slug>`). Panels
//! point at their page with `data-help="<slug>"` in `index.html`; the
//! tests below check both directions, and that every control named
//! between « » in a page exists in the interface.
//!
//! A page that describes a safety behaviour must change with it: any task
//! that changes arming, the e-stop, the interlocks or the safety stage
//! updates `securite-laser.html` (and its PR note says so).

pub struct HelpPage {
    pub slug: &'static str,
    pub title_fr: &'static str,
    /// Where the page sits in the table of contents: « Général » or a
    /// workspace (LIVE, TIMELINE, CRÉATION, RÉGLAGES). Workspace pages
    /// belong to a panel and are opened by its « ? ».
    pub group_fr: &'static str,
    pub body_html: &'static str,
}

/// The table of contents' general group (pages not tied to one panel).
pub const GENERAL: &str = "Général";

#[rustfmt::skip]
pub const PAGES: &[HelpPage] = &[
    HelpPage { slug: "premier-show", title_fr: "Premier show avec MadMapper et le ShowNET", group_fr: GENERAL, body_html: include_str!("premier-show.html") },
    HelpPage { slug: "securite-laser", title_fr: "Sécurité laser : armer, arrêter, réinitialiser", group_fr: GENERAL, body_html: include_str!("securite-laser.html") },
    HelpPage { slug: "depannage", title_fr: "Dépannage", group_fr: GENERAL, body_html: include_str!("depannage.html") },
    HelpPage { slug: "tempo", title_fr: "Tempo", group_fr: GENERAL, body_html: include_str!("tempo.html") },
    HelpPage { slug: "projet", title_fr: "Projet et fichiers", group_fr: GENERAL, body_html: include_str!("projet.html") },
    HelpPage { slug: "raccourcis", title_fr: "Raccourcis clavier", group_fr: GENERAL, body_html: include_str!("raccourcis.html") },
    HelpPage { slug: "live-direct", title_fr: "Direct", group_fr: "LIVE", body_html: include_str!("live-direct.html") },
    HelpPage { slug: "live-modulateurs", title_fr: "Modulateurs et liens audio", group_fr: "LIVE", body_html: include_str!("live-modulateurs.html") },
    HelpPage { slug: "live-effets", title_fr: "Grille d'effets", group_fr: "LIVE", body_html: include_str!("live-effets.html") },
    HelpPage { slug: "live-musique", title_fr: "Musique", group_fr: "LIVE", body_html: include_str!("live-musique.html") },
    HelpPage { slug: "live-scenes", title_fr: "Scènes", group_fr: "LIVE", body_html: include_str!("live-scenes.html") },
    HelpPage { slug: "live-prereglages", title_fr: "Préréglages", group_fr: "LIVE", body_html: include_str!("live-prereglages.html") },
    HelpPage { slug: "live-liste-cues", title_fr: "Liste de cues", group_fr: "LIVE", body_html: include_str!("live-liste-cues.html") },
    HelpPage { slug: "live-en-cours", title_fr: "En cours", group_fr: "LIVE", body_html: include_str!("live-en-cours.html") },
    HelpPage { slug: "live-cues", title_fr: "Grille de cues", group_fr: "LIVE", body_html: include_str!("live-cues.html") },
    HelpPage { slug: "live-macros", title_fr: "Macros", group_fr: "LIVE", body_html: include_str!("live-macros.html") },
    HelpPage { slug: "live-calques", title_fr: "Calques", group_fr: "LIVE", body_html: include_str!("live-calques.html") },
    HelpPage { slug: "timeline-shows", title_fr: "Shows", group_fr: "TIMELINE", body_html: include_str!("timeline-shows.html") },
    HelpPage { slug: "timeline-editeur", title_fr: "Éditeur de timeline", group_fr: "TIMELINE", body_html: include_str!("timeline-editeur.html") },
    HelpPage { slug: "timeline-timecode", title_fr: "Timecode", group_fr: "TIMELINE", body_html: include_str!("timeline-timecode.html") },
    HelpPage { slug: "timeline-lecteur", title_fr: "Lecteur de shows", group_fr: "TIMELINE", body_html: include_str!("timeline-lecteur.html") },
    HelpPage { slug: "creation-look", title_fr: "Look", group_fr: "CRÉATION", body_html: include_str!("creation-look.html") },
    HelpPage { slug: "creation-abstraits", title_fr: "Abstraits", group_fr: "CRÉATION", body_html: include_str!("creation-abstraits.html") },
    HelpPage { slug: "creation-figures", title_fr: "Figures", group_fr: "CRÉATION", body_html: include_str!("creation-figures.html") },
    HelpPage { slug: "creation-import", title_fr: "Importer un SVG ou une image", group_fr: "CRÉATION", body_html: include_str!("creation-import.html") },
    HelpPage { slug: "creation-macros", title_fr: "Macros", group_fr: "CRÉATION", body_html: include_str!("creation-macros.html") },
    HelpPage { slug: "creation-mediatheque", title_fr: "Médiathèque ILDA", group_fr: "CRÉATION", body_html: include_str!("creation-mediatheque.html") },
    HelpPage { slug: "creation-evolutif", title_fr: "Cue évolutif", group_fr: "CRÉATION", body_html: include_str!("creation-evolutif.html") },
    HelpPage { slug: "reglages-avant-show", title_fr: "Avant le show", group_fr: "RÉGLAGES", body_html: include_str!("reglages-avant-show.html") },
    HelpPage { slug: "reglages-securite", title_fr: "Sécurité", group_fr: "RÉGLAGES", body_html: include_str!("reglages-securite.html") },
    HelpPage { slug: "reglages-calibration", title_fr: "Calibration", group_fr: "RÉGLAGES", body_html: include_str!("reglages-calibration.html") },
    HelpPage { slug: "reglages-midi", title_fr: "Contrôleur MIDI", group_fr: "RÉGLAGES", body_html: include_str!("reglages-midi.html") },
    HelpPage { slug: "reglages-entrees", title_fr: "Entrées externes et réseau", group_fr: "RÉGLAGES", body_html: include_str!("reglages-entrees.html") },
    HelpPage { slug: "reglages-sorties", title_fr: "Sorties et projecteurs", group_fr: "RÉGLAGES", body_html: include_str!("reglages-sorties.html") },
];

/// A slug is `[a-z0-9-]`, 1 to 64 characters.
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty() && slug.len() <= 64 && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The page named `slug`, if it exists (an invalid slug finds nothing).
pub fn page(slug: &str) -> Option<&'static HelpPage> {
    if !valid_slug(slug) {
        return None;
    }
    PAGES.iter().find(|p| p.slug == slug)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const INDEX_HTML: &str = include_str!("../index.html");

    /// Every value of `attr="…"` in `html`.
    fn attr_values(html: &str, attr: &str) -> Vec<String> {
        let needle = format!("{attr}=\"");
        html.match_indices(&needle)
            .filter_map(|(i, _)| {
                let rest = &html[i + needle.len()..];
                rest.find('"').map(|end| rest[..end].to_string())
            })
            .collect()
    }

    /// The texts quoted « like this » in `html`.
    fn quoted(html: &str) -> Vec<String> {
        html.split('«').skip(1).filter_map(|s| s.split_once('»').map(|(q, _)| q.trim().to_string())).collect()
    }

    #[test]
    fn slugs_are_unique_valid_and_found() {
        let mut seen = BTreeSet::new();
        for p in PAGES {
            assert!(valid_slug(p.slug), "{}", p.slug);
            assert!(seen.insert(p.slug), "duplicate {}", p.slug);
            assert_eq!(page(p.slug).map(|f| f.title_fr), Some(p.title_fr));
            assert!(!p.title_fr.is_empty() && !p.body_html.trim().is_empty(), "{}", p.slug);
        }
        for bad in ["", "Live-direct", "../index", "live direct", "live_direct", "inconnue", &"a".repeat(65)] {
            assert!(page(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn pages_hold_no_script_and_nothing_external() {
        for p in PAGES {
            let lower = p.body_html.to_lowercase();
            for banned in ["http", "//", "<script", "<iframe", "<img", "<link", "<object", "<embed", "<style", "javascript:", "src=", "url("] {
                assert!(!lower.contains(banned), "{}: contains {banned:?}", p.slug);
            }
            // No event handler attribute (onclick=…).
            for (i, _) in lower.match_indices("=\"") {
                let name: String = lower[..i].chars().rev().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>().chars().rev().collect();
                assert!(!name.starts_with("on"), "{}: handler attribute {name}", p.slug);
            }
        }
    }

    #[test]
    fn links_between_pages_are_internal_and_lead_somewhere() {
        for p in PAGES {
            for href in attr_values(p.body_html, "href") {
                let slug = href.strip_prefix("#aide/").unwrap_or_else(|| panic!("{}: external or odd link {href}", p.slug));
                assert!(page(slug).is_some(), "{}: link to unknown page {slug}", p.slug);
            }
        }
    }

    #[test]
    fn every_panel_help_button_has_a_page_and_every_panel_page_a_panel() {
        let used: BTreeSet<String> = attr_values(INDEX_HTML, "data-help").into_iter().filter(|s| !s.contains('$')).collect();
        assert!(!used.is_empty());
        for slug in &used {
            assert!(page(slug).is_some(), "data-help=\"{slug}\" has no page");
        }
        for p in PAGES.iter().filter(|p| p.group_fr != GENERAL) {
            assert!(used.contains(p.slug), "panel page {} is reached by no data-help", p.slug);
        }
    }

    /// A label quoted in a page is on screen: in `index.html`, or one of
    /// the checklist items the studio sends to it (checklist.rs).
    #[test]
    fn controls_named_in_pages_exist_in_the_interface() {
        let items: Vec<String> = crate::checklist::shownet_items().into_iter().map(|i| i.text_fr).collect();
        for p in PAGES {
            for label in quoted(p.body_html) {
                assert!(INDEX_HTML.contains(&label) || items.contains(&label), "{}: « {label} » is not in the interface", p.slug);
            }
        }
    }

    #[test]
    fn the_safety_page_says_what_software_cannot_do() {
        let body = page("securite-laser").unwrap().body_html;
        for must in ["désarmé", "Échap", "verrouillé", "Réinitialiser l'arrêt d'urgence", "ne remplace ni", "réglementation", "Arrêt matériel testé", "Public"] {
            assert!(body.contains(must), "securite-laser misses {must:?}");
        }
        let trouble = page("depannage").unwrap().body_html;
        for must in ["ne s'arme pas", "Pas de son", "MIDI", "aperçu", "projet"] {
            assert!(trouble.contains(must), "depannage misses {must:?}");
        }
    }
}
