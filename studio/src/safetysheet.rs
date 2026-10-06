//! « Fiche sécurité du show » (T-263): a printable technical annex that
//! sums up, at the time of the export, what Laser Studio is configured to
//! do — projector sheets and caps (T-254), outdoor sectors (T-262), zones
//! and horizon (T-003, T-255), the static beam guard (T-256), the strobe
//! limiter (T-101), presence (T-252), the checklist's last run (T-258),
//! the exposure estimate (T-257), the active safety and site profiles
//! (T-260, T-289) and, if the operator asks, today's safety log (T-259).
//!
//! It is a summary of settings, never a statement that the show is
//! harmless or compliant: the page says plainly, at the top and at the
//! bottom, that it replaces neither a qualified assessment, nor the local
//! authorisation, nor measurements, nor the hardware stop. Exposure
//! numbers are labelled « estimation ».
//!
//! Generated locally as one self-contained HTML page: an inline print
//! stylesheet (A4), an inline SVG of the zones, no script, no external
//! font, image or link (the route also sends a CSP that forbids them).
//! The browser prints it to PDF. It is an annex the operator attaches to
//! a file; it does not imitate any official form.
//!
//! Reading only: `gather` copies what it needs from `Shared`, `render` is
//! pure. Nothing is saved — the event details typed for the sheet (and
//! the mobile number in particular) are written to no file and no log;
//! the page keeps the event fields in the browser, never the number. No
//! path of the machine goes into the sheet.

use crate::mpe::ExposureReport;
use crate::outdoor::{Mounting, OutdoorSettings};
use crate::power::{OutputConfig, MAIN_OUTPUT};
use crate::presence::PresenceSettings;
use crate::safety::SafetySettings;
use crate::safety_log::SafetyEvent;
use crate::zones::{Zone, ZoneKind};
use crate::Shared;
use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The disclaimer, at the top of the sheet.
pub const DISCLAIMER_FR: &str = "Ce document résume les réglages de Laser Studio au moment de l'export. Il n'atteste ni la sécurité ni la conformité du show, et ne remplace ni l'évaluation des risques par une personne compétente, ni l'autorisation requise (DGTA pour un tir en extérieur, autorités locales), ni des mesures sur place, ni l'arrêt matériel (arrêt d'urgence, verrouillage, clé du projecteur).";
/// The footer of every sheet (task T-263).
pub const FOOTER_FR: &str = "Document généré par Laser Studio. Ne remplace ni une mesure ni l'avis d'un expert.";
/// Under the software stop: it is not the operator's real stop.
pub const HW_STOP_FR: &str = "L'arrêt d'urgence logiciel (Échap) coupe la sortie de Laser Studio ; il ne remplace pas l'arrêt matériel du projecteur (arrêt d'urgence câblé, verrouillage, clé), qui reste l'arrêt de référence.";

/// What the sheet may load: nothing but its own inline styles and images.
pub const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; form-action 'none'; base-uri 'none'";

/// Longest event field, characters.
pub const MAX_FIELD: usize = 200;
/// Most event dates.
pub const MAX_DATES: usize = 10;
/// Most safety log events printed.
pub const MAX_LOG: usize = 200;

/// One day of the event: date, start, end, as typed (free text).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EventDate {
    pub day: String,
    pub start: String,
    pub end: String,
}

/// What the operator types in the sheet's form. Never saved by the
/// studio; `phone` is the person present during the show (DGTA form).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EventInfo {
    pub name: String,
    pub venue: String,
    pub dates: Vec<EventDate>,
    pub operator: String,
    pub phone: String,
}

/// What goes into the sheet besides the settings (`POST /api/safety/sheet`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SheetRequest {
    pub event: EventInfo,
    /// Today's safety log, as a table.
    pub include_log: bool,
    /// The free notes of the projector sheets (model, serial number...).
    pub include_notes: bool,
}

fn check_field(s: &str, what: &str) -> Result<()> {
    if s.chars().count() > MAX_FIELD {
        bail!("{what} : {MAX_FIELD} caractères au plus");
    }
    if s.chars().any(char::is_control) {
        bail!("{what} : caractères invalides");
    }
    Ok(())
}

impl EventInfo {
    pub fn validate(&self) -> Result<()> {
        check_field(&self.name, "Nom de l'événement")?;
        check_field(&self.venue, "Lieu")?;
        check_field(&self.operator, "Opérateur")?;
        check_field(&self.phone, "Téléphone")?;
        if self.dates.len() > MAX_DATES {
            bail!("{MAX_DATES} dates au plus");
        }
        for d in &self.dates {
            check_field(&d.day, "Date")?;
            check_field(&d.start, "Heure de début")?;
            check_field(&d.end, "Heure de fin")?;
        }
        Ok(())
    }
}

/// One projector of the venue, with its output's sheet and caps.
#[derive(Clone, Debug)]
pub struct ProjectorEntry {
    pub id: String,
    pub name: String,
    /// Height of the source in the 3D venue, m.
    pub venue_height_m: f32,
    pub config: OutputConfig,
    /// Outdoor mounting (T-262), when one is declared for it.
    pub mounting: Option<Mounting>,
    pub exposure: ExposureReport,
}

/// Everything the sheet shows, copied from `Shared` at export time.
#[derive(Clone, Debug)]
pub struct SheetData {
    pub generated_at: SystemTime,
    pub version: &'static str,
    pub commit: Option<&'static str>,
    pub build_date: &'static str,
    pub safety: SafetySettings,
    pub projectors: Vec<ProjectorEntry>,
    pub presence: PresenceSettings,
    pub presence_enforced: bool,
    pub estop_latched: bool,
    pub armed: bool,
    pub audience_scanning: bool,
    pub sky_off: bool,
    /// `Checklist::view()`.
    pub checklist: Value,
    pub safety_profile: Option<String>,
    pub safety_profile_modified: bool,
    pub site_profile: Option<String>,
    /// Today's safety log, when asked for.
    pub log: Option<Vec<SafetyEvent>>,
}

/// Copies what the sheet needs. Reads only; the safety log is read by the
/// caller, without the lock.
pub fn gather(s: &Shared, now: SystemTime) -> SheetData {
    let safety = s.safety.get();
    let venue = s.venue.get();
    let mut list: Vec<(String, String, f32)> = venue.projectors.iter().map(|p| (p.id.clone(), p.name.clone(), p.pos_m[1])).collect();
    if list.is_empty() {
        list.push((MAIN_OUTPUT.into(), "Projecteur 1".into(), 0.0));
    }
    let projectors = list
        .into_iter()
        .map(|(id, name, h)| {
            let config = s.outputs.config_for(&id);
            let mounting = if safety.outdoor.enabled { safety.outdoor.mounting_of(&id).copied() } else { None };
            let exposure = crate::mpe::report(&config.projector, &config.limits, &config.exposure);
            ProjectorEntry { id, name, venue_height_m: h, config, mounting, exposure }
        })
        .collect();
    SheetData {
        generated_at: now,
        version: crate::about::VERSION,
        commit: crate::about::commit(),
        build_date: crate::about::BUILD_DATE,
        safety,
        projectors,
        presence: s.presence.settings.clone(),
        presence_enforced: s.presence.enforced,
        estop_latched: s.estop.is_latched(),
        armed: s.gate.is_armed(),
        audience_scanning: s.audience.is_scanning(),
        sky_off: s.sky_off,
        checklist: s.checklist.view(),
        safety_profile: s.profiles.active().map(|p| p.name.clone()),
        safety_profile_modified: crate::profiles::modified(s),
        site_profile: s.sites.active().map(str::to_string),
        log: None,
    }
}

/// HTML-escaped text.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A typed field, or a blank line to fill by hand on the printout.
fn typed(s: &str) -> String {
    if s.trim().is_empty() {
        "<span class=\"blank\">à compléter</span>".into()
    } else {
        esc(s.trim())
    }
}

/// A number in French notation (comma), at most `digits` decimals.
fn num(v: f64, digits: usize) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let s = format!("{v:.digits$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    let s = if s == "-0" { "0".into() } else { s };
    s.replace('.', ",")
}

fn pct(v: f32) -> String {
    format!("{} %", num(f64::from(v) * 100.0, 0))
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "oui"
    } else {
        "non"
    }
}

fn local_time(t: SystemTime) -> String {
    crate::safety_log::local(t).strftime("%d/%m/%Y à %H:%M (UTC%:z)").to_string()
}

fn from_ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

const COLOURS: [&str; 3] = ["Rouge", "Vert", "Bleu"];

/// The kind of a zone as the sheet names it.
pub fn zone_kind_fr(z: &Zone) -> &'static str {
    match (z.audience, z.kind) {
        (true, ZoneKind::Blank) => "Public (coupée)",
        (true, ZoneKind::Dim) => "Public (atténuée)",
        (false, ZoneKind::Blank) => "Coupée (Blank)",
        (false, ZoneKind::Dim) => "Atténuée (Dim)",
    }
}

/// The preview field with the zones, the horizon and a legend, as inline
/// SVG (y up, the preview's orientation). Print colours, no script.
pub fn zones_svg(cfg: &SafetySettings) -> String {
    // Field -1..1 drawn in 0..400, y flipped.
    let px = |v: f32| f64::from(v.clamp(-1.2, 1.2) + 1.0) * 200.0;
    let py = |v: f32| f64::from(1.0 - v.clamp(-1.2, 1.2)) * 200.0;
    let mut out = String::from(
        "<svg class=\"zones\" viewBox=\"-10 -10 420 420\" width=\"420\" height=\"420\" role=\"img\" aria-labelledby=\"zones-title\" xmlns=\"http://www.w3.org/2000/svg\">\
<title id=\"zones-title\">Champ de projection avec les zones et l'horizon</title>\
<defs><pattern id=\"hatch\" width=\"8\" height=\"8\" patternUnits=\"userSpaceOnUse\" patternTransform=\"rotate(45)\"><rect width=\"8\" height=\"8\" fill=\"#d9d9d9\"/><line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"8\" stroke=\"#555\" stroke-width=\"3\"/></pattern></defs>\
<rect x=\"0\" y=\"0\" width=\"400\" height=\"400\" fill=\"#fff\" stroke=\"#000\" stroke-width=\"1.5\"/>\
<line x1=\"200\" y1=\"0\" x2=\"200\" y2=\"400\" stroke=\"#bbb\" stroke-dasharray=\"4 4\"/><line x1=\"0\" y1=\"200\" x2=\"400\" y2=\"200\" stroke=\"#bbb\" stroke-dasharray=\"4 4\"/>",
    );
    // Horizon: everything below is blanked for beams (and for all content
    // when `lines`), shaded.
    let h = cfg.horizon;
    let hy = py(h.y);
    if h.y > -1.0 {
        let fill = if h.lines { "#9e9e9e" } else { "#e3e3e3" };
        let _ = write!(out, "<rect x=\"0\" y=\"{hy:.1}\" width=\"400\" height=\"{:.1}\" fill=\"{fill}\" fill-opacity=\"0.6\"/>", (400.0 - hy).max(0.0));
    }
    let _ = write!(out, "<line x1=\"0\" y1=\"{hy:.1}\" x2=\"400\" y2=\"{hy:.1}\" stroke=\"#000\" stroke-width=\"2\"/><text x=\"6\" y=\"{:.1}\" font-size=\"12\">Horizon</text>", (hy - 4.0).max(12.0));
    for z in &cfg.zones {
        if z.points.len() < 3 {
            continue;
        }
        let pts: Vec<String> = z.points.iter().map(|p| format!("{:.1},{:.1}", px(p[0]), py(p[1]))).collect();
        let (fill, opacity, stroke, dash) = match (z.audience, z.kind) {
            (true, _) => ("#f4b183", "0.55", "#b00000", " stroke-dasharray=\"8 4\""),
            (false, ZoneKind::Blank) => ("url(#hatch)", "0.9", "#000", ""),
            (false, ZoneKind::Dim) => ("#ffe08a", "0.6", "#8a6d00", ""),
        };
        let _ = write!(out, "<polygon points=\"{}\" fill=\"{fill}\" fill-opacity=\"{opacity}\" stroke=\"{stroke}\" stroke-width=\"2\"{dash}/>", pts.join(" "));
        let n = z.points.len() as f32;
        let cx = z.points.iter().map(|p| p[0]).sum::<f32>() / n;
        let cy = z.points.iter().map(|p| p[1]).sum::<f32>() / n;
        let _ = write!(
            out,
            "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"12\" text-anchor=\"middle\" paint-order=\"stroke\" stroke=\"#fff\" stroke-width=\"3\">{}</text>",
            px(cx),
            py(cy),
            esc(&z.name)
        );
    }
    out.push_str("</svg>");
    out
}

/// One `<tr><th>…</th><td>…</td></tr>`; `value` is already HTML.
fn row(out: &mut String, label: &str, value: &str) {
    let _ = write!(out, "<tr><th scope=\"row\">{}</th><td>{value}</td></tr>", esc(label));
}

const STYLE: &str = "\
@page{size:A4;margin:15mm}\
*{box-sizing:border-box}\
body{margin:0 auto;max-width:190mm;padding:12px;background:#fff;color:#000;font:11pt/1.4 -apple-system,'Helvetica Neue',Arial,sans-serif}\
h1{font-size:17pt;margin:0 0 4px}h2{font-size:13pt;margin:18px 0 6px;border-bottom:1.5px solid #000;padding-bottom:2px;break-after:avoid}\
h3{font-size:11.5pt;margin:12px 0 4px;break-after:avoid}\
table{border-collapse:collapse;width:100%;margin:4px 0 8px;break-inside:auto}\
th,td{border:1px solid #888;padding:3px 6px;text-align:left;vertical-align:top}\
th{background:#eee;font-weight:600}th[scope=row]{width:42%}\
tr{break-inside:avoid}\
.meta{font-size:9.5pt;color:#333}\
.warn{border:2px solid #000;padding:8px 10px;margin:10px 0;font-weight:600}\
.note{font-size:9.5pt;color:#333}\
.est{font-style:italic}\
.blank{display:inline-block;min-width:60mm;border-bottom:1px dotted #000;color:#777;font-size:8.5pt}\
.zones{display:block;max-width:100%;height:auto;margin:6px 0;break-inside:avoid}\
.legend span{display:inline-block;margin-right:14px;font-size:9.5pt}\
.sw{display:inline-block;width:14px;height:10px;border:1px solid #000;margin-right:4px;vertical-align:middle}\
footer{margin-top:20px;border-top:1.5px solid #000;padding-top:6px;font-size:9.5pt}\
section{break-inside:auto}\
";

/// The whole sheet, one self-contained HTML page without script.
pub fn render(d: &SheetData, req: &SheetRequest) -> String {
    let e = &req.event;
    let mut o = String::with_capacity(32 * 1024);
    o.push_str("<!DOCTYPE html><html lang=\"fr\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    // The same policy as the route's header, for the copy the page opens
    // from memory (a blob URL gets no header).
    let _ = write!(o, "<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">");
    let title = if e.name.trim().is_empty() { "Fiche sécurité du show".to_string() } else { format!("Fiche sécurité — {}", e.name.trim()) };
    let _ = write!(o, "<title>{}</title><style>{STYLE}</style></head><body>", esc(&title));

    // Header.
    o.push_str("<header><h1>Fiche sécurité du show — annexe technique</h1>");
    let commit = d.commit.map(|c| format!(", commit {}", esc(c))).unwrap_or_default();
    let _ = write!(
        o,
        "<p class=\"meta\">Réglages du logiciel au moment de l'export : {}. Laser Studio {}{commit} (compilé le {}).</p>",
        esc(&local_time(d.generated_at)),
        esc(d.version),
        esc(d.build_date)
    );
    let _ = write!(o, "<p class=\"warn\" role=\"note\">{}</p></header>", esc(DISCLAIMER_FR));

    // 1. Event.
    o.push_str("<section><h2>1. Événement</h2><table>");
    row(&mut o, "Nom de l'événement", &typed(&e.name));
    row(&mut o, "Lieu (adresse)", &typed(&e.venue));
    row(&mut o, "Opérateur laser", &typed(&e.operator));
    row(&mut o, "Personne présente pendant la projection, joignable sur portable", &typed(&e.phone));
    o.push_str("</table><h3>Dates et heures</h3><table><thead><tr><th scope=\"col\">Date</th><th scope=\"col\">Début</th><th scope=\"col\">Fin</th></tr></thead><tbody>");
    let dates: Vec<&EventDate> = e.dates.iter().filter(|d| !(d.day.trim().is_empty() && d.start.trim().is_empty() && d.end.trim().is_empty())).collect();
    if dates.is_empty() {
        o.push_str("<tr><td><span class=\"blank\"></span></td><td><span class=\"blank\"></span></td><td><span class=\"blank\"></span></td></tr>");
    }
    for dt in dates {
        let _ = write!(o, "<tr><td>{}</td><td>{}</td><td>{}</td></tr>", typed(&dt.day), typed(&dt.start), typed(&dt.end));
    }
    o.push_str("</tbody></table>");
    let profile = match &d.safety_profile {
        Some(n) => format!("{}{}", esc(n), if d.safety_profile_modified { " (modifié depuis son chargement)" } else { "" }),
        None => "aucun".into(),
    };
    o.push_str("<table>");
    row(&mut o, "Profil de sécurité actif", &profile);
    row(&mut o, "Profil de site actif", &d.site_profile.as_deref().map(esc).unwrap_or_else(|| "aucun".into()));
    o.push_str("</table></section>");

    // 2. Projectors.
    o.push_str("<section><h2>2. Projecteurs et sorties</h2>");
    for p in &d.projectors {
        let pi = &p.config.projector;
        let l = &p.config.limits;
        let label = if pi.name.trim().is_empty() { p.name.clone() } else { format!("{} — {}", p.name, pi.name.trim()) };
        let _ = write!(o, "<h3>{} <span class=\"note\">(sortie « {} »)</span></h3><table>", esc(&label), esc(&p.id));
        row(&mut o, "Classe (IEC 60825-1)", &if pi.class.is_empty() { "non renseignée".into() } else { esc(&pi.class) });
        row(&mut o, "Mode d'émission", "non renseigné dans Laser Studio (onde entretenue / impulsionnel : à indiquer d'après le fabricant)");
        row(&mut o, "Diamètre du faisceau à la sortie", &if pi.aperture_mm > 0.0 { format!("{} cm ({} mm)", num(f64::from(pi.aperture_mm) / 10.0, 2), num(f64::from(pi.aperture_mm), 1)) } else { "non renseigné".into() });
        row(&mut o, "Divergence (angle total)", &if pi.divergence_mrad > 0.0 { format!("{} mrad", num(f64::from(pi.divergence_mrad), 2)) } else { "non renseignée".into() });
        row(&mut o, "Lentille de divergence", yes_no(pi.divergence_lens));
        row(&mut o, "Angle de balayage optique", &format!("{} °", num(f64::from(pi.scan_angle_deg), 1)));
        row(&mut o, "Protection matérielle contre l'arrêt du balayage", match pi.hw_scan_fail {
            Some(true) => "oui (déclarée)",
            Some(false) => "non",
            None => "inconnue",
        });
        row(&mut o, "Interface (DAC)", &esc(pi.dac_model.preset().label_fr));
        row(&mut o, "Plafond global de la sortie", &pct(l.max_power));
        o.push_str("</table><table><thead><tr><th scope=\"col\">Couleur</th><th scope=\"col\">Longueur d'onde (nm)</th><th scope=\"col\">Puissance max (W)</th><th scope=\"col\">Plafond couleur</th><th scope=\"col\">Puissance max après plafonds (W)</th></tr></thead><tbody>");
        for (i, c) in COLOURS.iter().enumerate() {
            let w = pi.wavelength_nm[i];
            let mw = f64::from(pi.power_mw[i]);
            let (power, capped) = if mw > 0.0 {
                (num(mw / 1000.0, 3), num(mw * f64::from(l.ceiling(i)) / 1000.0, 3))
            } else {
                ("non renseignée".to_string(), "—".to_string())
            };
            let _ = write!(
                o,
                "<tr><td>{c}</td><td>{}</td><td>{power}</td><td>{}</td><td>{capped}</td></tr>",
                if w > 0 { w.to_string() } else { "non renseignée".into() },
                pct(l.max_color[i])
            );
        }
        o.push_str("</tbody></table><table>");
        row(&mut o, "Hauteur de la source (vue 3D du lieu)", &format!("{} m", num(f64::from(p.venue_height_m), 2)));
        match &p.mounting {
            Some(m) => {
                row(&mut o, "Montage extérieur : hauteur au-dessus du sol", &format!("{} m", num(f64::from(m.height_m), 2)));
                row(&mut o, "Montage extérieur : azimut de l'axe (0 = nord)", &format!("{} °", num(f64::from(m.azimuth_deg), 1)));
                row(&mut o, "Montage extérieur : élévation de l'axe", &format!("{} °", num(f64::from(m.tilt_deg), 1)));
            }
            None if d.safety.outdoor.enabled => row(&mut o, "Montage extérieur", "aucun déclaré : ce projecteur est entièrement coupé en mode extérieur"),
            None => {}
        }
        if req.include_notes && !pi.notes.trim().is_empty() {
            row(&mut o, "Notes de la fiche projecteur", &esc(pi.notes.trim()));
        }
        o.push_str("</table>");
    }
    o.push_str("<p class=\"note\">Valeurs de la fiche projecteur saisies par l'opérateur (RÉGLAGES › Sorties et projecteurs), non mesurées par le logiciel. Les plafonds limitent ce que Laser Studio envoie ; ils ne limitent pas le projecteur piloté par un autre moyen.</p></section>");

    // 3. Outdoor.
    let out = &d.safety.outdoor;
    o.push_str("<section><h2>3. Extérieur</h2><table>");
    row(&mut o, "Mode « Show en extérieur »", if out.enabled { "activé" } else { "désactivé" });
    row(&mut o, "Référence d'autorisation DGTA", &typed(&out.authorization_ref));
    row(&mut o, "Coordonnées WGS84 de la source", &match out.wgs84 {
        Some((lat, lon)) => format!("latitude {}, longitude {}", num(lat, 6), num(lon, 6)),
        None => typed(""),
    });
    row(&mut o, "Limite : écart max à la verticale", &vertical_limit(out));
    row(&mut o, "« Ciel coupé » au moment de l'export", if d.sky_off { "oui" } else { "non" });
    o.push_str("</table><h3>Faisceaux déclarés</h3>");
    if out.sectors.is_empty() {
        o.push_str("<p>Aucun secteur déclaré.</p>");
    } else {
        o.push_str("<table><thead><tr><th scope=\"col\">Faisceau n°</th><th scope=\"col\">Angle horizontal de – à (°)</th><th scope=\"col\">Angle vertical de – à (°)</th></tr></thead><tbody>");
        for (i, s) in out.sectors.iter().enumerate() {
            let _ = write!(
                o,
                "<tr><td>{}</td><td>{} – {}</td><td>{} – {}</td></tr>",
                i + 1,
                num(f64::from(s.az_min), 1),
                num(f64::from(s.az_max), 1),
                num(f64::from(s.el_min), 1),
                num(f64::from(s.el_max), 1)
            );
        }
        o.push_str("</tbody></table><p class=\"note\">Angle horizontal dans le sens horaire depuis le nord ; angle vertical depuis l'horizontale.</p>");
    }
    o.push_str("</section>");

    // 4. Safety measures.
    let s = &d.safety;
    o.push_str("<section><h2>4. Mesures de sécurité configurées</h2><h3>Zones et horizon (aperçu)</h3>");
    o.push_str(&zones_svg(s));
    o.push_str("<p class=\"legend\"><span><i class=\"sw\" style=\"background:repeating-linear-gradient(45deg,#555 0 3px,#d9d9d9 3px 8px)\"></i>Coupée (Blank)</span><span><i class=\"sw\" style=\"background:#ffe08a\"></i>Atténuée (Dim)</span><span><i class=\"sw\" style=\"background:#f4b183;border:2px dashed #b00000\"></i>Public</span><span><i class=\"sw\" style=\"background:#e3e3e3\"></i>Sous l'horizon</span></p>");
    if s.zones.is_empty() {
        o.push_str("<p>Aucune zone définie.</p>");
    } else {
        o.push_str("<table><thead><tr><th scope=\"col\">Zone</th><th scope=\"col\">Type</th><th scope=\"col\">Niveau</th><th scope=\"col\">Sommets</th><th scope=\"col\">Fondu</th></tr></thead><tbody>");
        for z in &s.zones {
            let level = match z.kind {
                ZoneKind::Blank => "0 %".to_string(),
                ZoneKind::Dim => pct(z.level),
            };
            let _ = write!(o, "<tr><td>{}</td><td>{}</td><td>{level}</td><td>{}</td><td>{}</td></tr>", esc(&z.name), zone_kind_fr(z), z.points.len(), num(f64::from(z.feather), 2));
        }
        o.push_str("</tbody></table>");
    }
    let public = if d.audience_scanning {
        "Balayage du public déverrouillé : les zones Public suivent leur type (au plus le niveau autorisé)."
    } else {
        "« Au-dessus du public » : toute zone Public est coupée comme une zone Blank."
    };
    o.push_str("<table>");
    row(&mut o, "Zones Public", public);
    row(&mut o, "Horizon (coordonnées de sortie, -1 bas à 1 haut)", &format!("{}{}", num(f64::from(s.horizon.y), 2), if s.horizon.lines { " — appliqué à tout le contenu (lignes, textes, nappes)" } else { " — faisceaux fixes seulement" }));
    row(&mut o, "Gain par couleur (R, V, B)", &format!("{} / {} / {}", pct(s.color_gain[0]), pct(s.color_gain[1]), pct(s.color_gain[2])));
    row(&mut o, "Limiteur de stroboscope", &format!("{} flashs/s au plus en continu, rafale de {} s au plus, puis {} s de sortie stable", num(f64::from(s.strobe_max_hz), 1), num(f64::from(s.strobe_burst_s), 1), num(f64::from(s.strobe_cooldown_s), 1)));
    let dw = &s.dwell;
    let profile = match dw.profile {
        crate::dwell::DwellProfile::Strict => "strict partout",
        crate::dwell::DwellProfile::Beams => "faisceaux fixes autorisés au-dessus de l'horizon sous un plafond, strict en dessous",
        crate::dwell::DwellProfile::Off => "désactivé à l'arrêt (armé : profil faisceaux)",
    };
    row(&mut o, "Garde anti-point fixe", &format!("{profile} ; fenêtre {} ms, dose max {} (faisceaux {})", dw.window_ms, pct(dw.max_cell_dose), pct(dw.max_cell_dose_beams)));
    row(&mut o, "Arrêt d'urgence logiciel", &format!("Échap, verrouillé jusqu'à réarmement{}", if d.estop_latched { " — déclenché au moment de l'export" } else { "" }));
    row(&mut o, "État de la sortie au moment de l'export", if d.armed { "armée" } else { "désarmée" });
    let presence = if d.presence_enforced {
        format!(
            "surveillée : désarmement si l'interface ne répond plus pendant {} s{}",
            num(f64::from(d.presence.ui_timeout_ms) / 1000.0, 1),
            if d.presence.hold_to_run { format!(" ; maintien obligatoire (touche {})", esc(&d.presence.hold_key)) } else { String::new() }
        )
    } else {
        "non surveillée (aperçu seul, aucune sortie réelle)".into()
    };
    row(&mut o, "Présence de l'opérateur", &presence);
    o.push_str("</table>");
    let _ = write!(o, "<p class=\"warn\" role=\"note\">{}</p>", esc(HW_STOP_FR));

    // Checklist.
    o.push_str("<h3>Liste de contrôle avant show</h3>");
    let cl = &d.checklist;
    let completed = cl.get("completed_at").and_then(Value::as_u64);
    o.push_str("<table>");
    row(&mut o, "Dernière exécution terminée (cette session)", &completed.map(|ms| esc(&local_time(from_ms(ms)))).unwrap_or_else(|| "aucune".into()));
    let operator = cl.get("operator").and_then(Value::as_str).unwrap_or("");
    if !operator.trim().is_empty() {
        row(&mut o, "Opérateur de la liste", &esc(operator));
    }
    row(&mut o, "Exigée avant d'armer", yes_no(cl.get("enforced").and_then(Value::as_bool).unwrap_or(false)));
    o.push_str("</table>");
    if let Some(items) = cl.get("items").and_then(Value::as_array) {
        o.push_str("<table><thead><tr><th scope=\"col\">Point</th><th scope=\"col\">Coché</th></tr></thead><tbody>");
        for it in items.iter().filter(|i| i.get("visible").and_then(Value::as_bool).unwrap_or(true)) {
            let text = it.get("text_fr").and_then(Value::as_str).unwrap_or("");
            let done = it.get("done").and_then(Value::as_bool).unwrap_or(false);
            let mut cell = if done { "oui".to_string() } else { "non".to_string() };
            if let Some(at) = it.get("at").and_then(Value::as_u64).filter(|_| done) {
                cell = format!("oui, {}", esc(&local_time(from_ms(at))));
            }
            if let Some(v) = it.get("value").and_then(Value::as_str).filter(|v| !v.is_empty()) {
                let _ = write!(cell, " — {}", esc(v));
            }
            let _ = write!(o, "<tr><td>{}</td><td>{cell}</td></tr>", esc(text));
        }
        o.push_str("</tbody></table>");
    }
    o.push_str("</section>");

    // 5. Exposure estimate.
    o.push_str("<section><h2>5. Estimation d'exposition</h2>");
    let mut any = false;
    for p in &d.projectors {
        let r = &p.exposure;
        let Some(res) = r.result else { continue };
        any = true;
        let _ = write!(o, "<h3>{} <span class=\"note\">(sortie « {} »)</span></h3><table class=\"est\">", esc(&p.name), esc(&p.id));
        row(&mut o, "Distance minimale du public", &format!("{} m", num(f64::from(r.settings.distance_m), 2)));
        row(&mut o, "Atténuation dans la zone public", &format!("{} %", num(f64::from(r.settings.attenuation_pct), 1)));
        row(&mut o, "Puissance après plafonds (estimation)", &format!("{} mW", num(f64::from(r.power_capped_mw), 1)));
        row(&mut o, "Diamètre du faisceau au public (estimation)", &format!("{} mm", num(f64::from(res.beam_mm), 1)));
        row(&mut o, "Éclairement (estimation)", &format!("{} mW/cm²", num(f64::from(res.irradiance_mw_cm2), 3)));
        row(&mut o, "EMP de référence", &format!("{} mW/cm² pour {} s", num(f64::from(res.mpe_mw_cm2), 3), num(f64::from(r.exposure_s), 2)));
        row(&mut o, "Rapport éclairement / EMP (estimation)", &num(f64::from(res.ratio), 2));
        row(&mut o, "Distance nominale de risque oculaire (estimation)", &res.nohd_m.map(|v| format!("{} m", num(f64::from(v), 1))).unwrap_or_else(|| "aucune".into()));
        if let Some(full) = r.ratio_full {
            row(&mut o, "Rapport à pleine puissance, sans plafond logiciel (estimation)", &num(f64::from(full), 2));
        }
        o.push_str("</table>");
        let notes: Vec<&String> = r.warnings.iter().chain(&r.assumptions).collect();
        if !notes.is_empty() {
            o.push_str("<ul class=\"note\">");
            for n in notes {
                let _ = write!(o, "<li>{}</li>", esc(n));
            }
            o.push_str("</ul>");
        }
        let _ = write!(o, "<p class=\"note\">{}</p>", esc(r.disclaimer));
    }
    if !any {
        o.push_str("<p>Non renseignée : aucune puissance dans les fiches projecteurs.</p>");
    }
    o.push_str("</section>");

    // 6. Safety log (on request).
    if let Some(events) = &d.log {
        o.push_str("<section><h2>6. Journal de sécurité du jour</h2>");
        if events.is_empty() {
            o.push_str("<p>Aucun événement.</p>");
        } else {
            o.push_str("<table><thead><tr><th scope=\"col\">Heure</th><th scope=\"col\">Événement</th></tr></thead><tbody>");
            let skip = events.len().saturating_sub(MAX_LOG);
            for ev in &events[skip..] {
                let time = ev.ts.get(11..19).unwrap_or(&ev.ts);
                let _ = write!(o, "<tr><td>{}</td><td>{}</td></tr>", esc(time), esc(&crate::safety_log::describe(ev)));
            }
            o.push_str("</tbody></table>");
            if skip > 0 {
                let _ = write!(o, "<p class=\"note\">{skip} événement(s) plus anciens non imprimés.</p>");
            }
        }
        o.push_str("</section>");
    }

    let _ = write!(o, "<footer><p><strong>{}</strong></p><p>{}</p></footer></body></html>", esc(FOOTER_FR), esc(HW_STOP_FR));
    o
}

fn vertical_limit(out: &OutdoorSettings) -> String {
    let waived = out.vertical_limit_waived && !out.waiver_ref.trim().is_empty();
    if waived {
        format!("levée par l'autorisation ({})", esc(out.waiver_ref.trim()))
    } else {
        format!("{} °", num(f64::from(out.max_from_vertical_deg), 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outdoor::Sector;
    use crate::power::ProjectorInfo;
    use crate::test_support;
    use crate::zones::Horizon;

    fn zone(id: u32, name: &str, kind: ZoneKind, audience: bool, pts: &[[f32; 2]]) -> Zone {
        Zone { id, name: name.into(), kind, level: 0.3, points: pts.to_vec(), audience, ..Zone::default() }
    }

    /// A configured studio: three zones, a horizon, outdoor sectors, a
    /// projector sheet with powers and caps.
    fn configured() -> Shared {
        let mut s = test_support::shared();
        let mut cfg = SafetySettings {
            zones: vec![
                zone(1, "Scène <gauche>", ZoneKind::Blank, false, &[[-1.0, -1.0], [-0.5, -1.0], [-0.5, 0.0]]),
                zone(2, "Balcon", ZoneKind::Dim, false, &[[0.2, 0.2], [0.6, 0.2], [0.6, 0.6], [0.2, 0.6]]),
                zone(3, "Fosse", ZoneKind::Blank, true, &[[-0.4, -1.0], [0.4, -1.0], [0.4, -0.6], [-0.4, -0.6]]),
            ],
            horizon: Horizon { y: -0.3, ..Horizon::default() },
            ..SafetySettings::default()
        };
        cfg.outdoor.enabled = true;
        cfg.outdoor.authorization_ref = "DGTA-2026-042".into();
        cfg.outdoor.wgs84 = Some((50.8503, 4.3517));
        cfg.outdoor.sectors = vec![Sector { az_min: 350.0, az_max: 10.0, el_min: 20.0, el_max: 60.0 }];
        s.safety.set(cfg, true).expect("test settings");
        let projector = ProjectorInfo {
            name: "Mon RGB 3W".into(),
            class: "4".into(),
            power_mw: [1000.0, 1500.0, 500.0],
            wavelength_nm: [638, 520, 450],
            aperture_mm: 5.0,
            divergence_mrad: 1.2,
            notes: "Série 12345".into(),
            ..ProjectorInfo::default()
        };
        let limits = crate::power::OutputLimits { max_power: 0.4, max_color: [1.0, 0.5, 1.0] };
        s.outputs.set(MAIN_OUTPUT, Some(limits), Some(projector), false, true).expect("test sheet");
        s
    }

    fn sheet(s: &Shared, req: &SheetRequest) -> String {
        render(&gather(s, SystemTime::now()), req)
    }

    #[test]
    fn lists_every_zone_cap_and_the_disclaimer() {
        let s = configured();
        let html = sheet(&s, &SheetRequest::default());
        for name in ["Scène &lt;gauche&gt;", "Balcon", "Fosse"] {
            assert!(html.contains(name), "zone {name} missing");
        }
        assert!(!html.contains("Scène <gauche>"), "zone names are escaped");
        assert!(html.contains("Coupée (Blank)") && html.contains("Atténuée (Dim)") && html.contains("Public (coupée)"));
        // The SVG draws the three polygons and the horizon.
        assert_eq!(html.matches("<polygon").count(), 3);
        assert!(html.contains(">Horizon</text>"));
        // Caps: global 40 %, green clipped to 50 %; powers in W after caps.
        assert!(html.contains("Plafond global de la sortie</th><td>40 %"));
        assert!(html.contains("<td>Vert</td><td>520</td><td>1,5</td><td>50 %</td><td>0,3</td>"), "{html}");
        assert!(html.contains("<td>Rouge</td><td>638</td><td>1</td><td>100 %</td><td>0,4</td>"));
        // DGTA fields: class, diameter in cm, divergence, angles.
        assert!(html.contains("0,5 cm (5 mm)"));
        assert!(html.contains("1,2 mrad"));
        assert!(html.contains("<td>1</td><td>350 – 10</td><td>20 – 60</td>"));
        assert!(html.contains("DGTA-2026-042"));
        assert!(html.contains("latitude 50,8503, longitude 4,3517"));
        // Disclaimer, footer, hardware stop.
        assert!(html.contains(&esc(DISCLAIMER_FR)));
        assert!(html.contains(&esc(FOOTER_FR)));
        assert!(html.contains(&esc(HW_STOP_FR)));
        // The exposure numbers are labelled as estimates.
        assert!(html.contains("Éclairement (estimation)"));
        // Version and commit-free build info.
        assert!(html.contains(crate::about::VERSION));
    }

    #[test]
    fn makes_no_claim_and_has_no_script_or_external_resource() {
        let html = sheet(&configured(), &SheetRequest { include_log: true, include_notes: true, ..SheetRequest::default() });
        let lower = html.to_lowercase();
        for bad in ["<script", "javascript:", "http://", "https://", "@import", "<link", "src=", "certifi", " sûr", "conforme "] {
            let ok = if bad == "http://" { !lower.replace("http://www.w3.org/2000/svg", "").contains(bad) } else { !lower.contains(bad) };
            assert!(ok, "the sheet contains {bad:?}");
        }
    }

    #[test]
    fn no_machine_path_in_the_sheet() {
        let html = sheet(&configured(), &SheetRequest::default());
        assert!(!html.contains("/Users/") && !html.contains("/tmp/") && !html.contains(&std::env::temp_dir().display().to_string()));
        // Notes only when asked for.
        assert!(!html.contains("Série 12345"));
        let with = sheet(&configured(), &SheetRequest { include_notes: true, ..SheetRequest::default() });
        assert!(with.contains("Série 12345"));
    }

    #[test]
    fn event_fields_are_escaped_and_blank_ones_left_to_fill() {
        let s = configured();
        let blank = sheet(&s, &SheetRequest::default());
        assert!(blank.matches("à compléter").count() >= 4);
        let req = SheetRequest {
            event: EventInfo {
                name: "Fête <b>".into(),
                venue: "Place & co".into(),
                dates: vec![EventDate { day: "2026-10-10".into(), start: "21:00".into(), end: "23:30".into() }],
                operator: "A. Dupont".into(),
                phone: "+32 470 00 00 00".into(),
            },
            ..SheetRequest::default()
        };
        req.event.validate().unwrap();
        let html = sheet(&s, &req);
        assert!(html.contains("Fête &lt;b&gt;") && html.contains("Place &amp; co"));
        assert!(html.contains("<td>2026-10-10</td><td>21:00</td><td>23:30</td>"));
        assert!(html.contains("+32 470 00 00 00"));
    }

    #[test]
    fn event_validation_bounds_the_fields() {
        let long = EventInfo { name: "x".repeat(MAX_FIELD + 1), ..EventInfo::default() };
        assert!(long.validate().is_err());
        let ctrl = EventInfo { phone: "1\n2".into(), ..EventInfo::default() };
        assert!(ctrl.validate().is_err());
        let many = EventInfo { dates: vec![EventDate::default(); MAX_DATES + 1], ..EventInfo::default() };
        assert!(many.validate().is_err());
    }

    #[test]
    fn generating_the_sheet_changes_no_state() {
        let s = configured();
        let safety = s.safety.get();
        let outputs = s.outputs.list().to_vec();
        let checklist = s.checklist.view();
        let venue = s.venue.get().clone();
        let _ = sheet(&s, &SheetRequest { include_log: true, include_notes: true, ..SheetRequest::default() });
        assert!(!s.gate.is_armed(), "never arms");
        assert_eq!(s.safety.get(), safety);
        assert_eq!(s.outputs.list(), &outputs[..]);
        assert_eq!(s.checklist.view(), checklist);
        assert_eq!(s.venue.get(), &venue);
    }

    #[test]
    fn empty_studio_says_what_is_missing() {
        let s = test_support::shared();
        let html = sheet(&s, &SheetRequest::default());
        assert!(html.contains("Aucune zone définie."));
        assert!(html.contains("Aucun secteur déclaré."));
        assert!(html.contains("Non renseignée : aucune puissance"));
        assert_eq!(html.matches("<polygon").count(), 0);
    }

    #[test]
    fn numbers_are_french() {
        assert_eq!(num(1.5, 3), "1,5");
        assert_eq!(num(2.0, 2), "2");
        assert_eq!(num(-0.0001, 2), "0");
        assert_eq!(num(f64::NAN, 2), "—");
    }
}
