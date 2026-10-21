//! The relay's status page (`GET /`), for a quick look from a phone.

use kp_link::relay::RelayStatus;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn render(st: &RelayStatus) -> String {
    let state = if st.busy {
        let pct = (st.sent * 100).checked_div(st.total).unwrap_or(0);
        format!("<b class=busy>Découpe en cours</b> — {pct} %")
    } else if st.device_error.is_some() {
        "<b class=err>Plotter non détecté</b>".to_owned()
    } else {
        "<b class=ok>Prêt</b>".to_owned()
    };
    let device = match (&st.device_error, st.device.is_empty()) {
        (Some(e), _) => esc(e),
        (None, false) => esc(&st.device),
        (None, true) => "—".to_owned(),
    };
    let last = st.last.as_ref().map_or_else(
        || "Aucun".to_owned(),
        |l| {
            let what = if l.ok {
                "Envoyé".to_owned()
            } else if l.cancelled {
                "Annulé".to_owned()
            } else {
                format!("Erreur : {}", esc(l.error.as_deref().unwrap_or("?")))
            };
            format!("{what} ({} octets, depuis {})", l.bytes, esc(&l.client))
        },
    );
    let ports = if st.ports.is_empty() {
        "Aucun".to_owned()
    } else {
        st.ports
            .iter()
            .map(|p| {
                let d = p
                    .description
                    .as_deref()
                    .map(|d| format!(" — {}", esc(d)))
                    .unwrap_or_default();
                format!("{}{d}", esc(&p.name))
            })
            .collect::<Vec<_>>()
            .join("<br>")
    };
    format!(
        r#"<!doctype html><html lang="fr"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="refresh" content="2"><title>KI Plotter — {name}</title>
<style>
body{{font:15px/1.5 system-ui,sans-serif;background:#15181e;color:#d9dee7;margin:0;padding:24px 16px}}
main{{max-width:520px;margin:auto}}h1{{font-size:18px;margin:0 0 16px}}h1 span{{color:#ff8a3d}}
dl{{display:grid;grid-template-columns:auto minmax(0,1fr);gap:8px 16px;background:#20252e;padding:16px;border-radius:8px}}
dt{{color:#8a93a3}}dd{{margin:0;overflow-wrap:anywhere}}.ok{{color:#5ad17d}}.busy{{color:#ff8a3d}}.err{{color:#ff6b6f}}
footer{{color:#6b7484;font-size:12px;margin-top:12px}}
</style></head><body><main>
<h1><span>KI</span> Plotter — relais « {name} »</h1>
<dl><dt>État</dt><dd>{state}</dd><dt>Plotter</dt><dd>{device}</dd>
<dt>Dernier travail</dt><dd>{last}</dd><dt>Ports série</dt><dd>{ports}</dd></dl>
<footer>Version {version} · actualisé toutes les 2 s</footer>
</main></body></html>"#,
        name = esc(&st.name),
        version = esc(&st.version),
    )
}
