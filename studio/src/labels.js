// Table unique des libellés de la page (T-461).
//
// Every visible French text of the migrated parts of index.html comes from
// LABELS: the markup carries a key, never the text, and the scripts call
// libelle(). One place to fix a word, one vocabulary, and a table ready for
// another language (none is added here).
//
// - Keys: lower case, by domain, dot-separated, describing the role, not
//   the text (`securite.arret`, `projet.menu.ouvrir`). `[a-z0-9_]` only.
// - Parameters: `{nom}` in a value is replaced by params.nom
//   (libelle('x', { nom: 'A' })). A parameter missing from the call stays as is.
// - Plural: two keys `….un` and `….autre`; libelle('k', { n }) picks `k.un` for
//   n = 0 or 1 (French usage) and `k.autre` otherwise.
// - An unknown key gives `⟦key⟧` and a console warning: it shows at once.
// - Markup: data-t (content), data-t-title, data-t-placeholder,
//   data-t-aria-label; applyLabels() fills them in one pass at start-up.
//   An element with data-t-fallback keeps its text in the markup too (the
//   safety buttons, the e-stop banner, the alerts: shown even if this file
//   does not load); the tests check that text equals the table's.
// - If this file does not load, index.html's LABELS_FALLBACK (the safety
//   and alert texts, equal to this table: tests/ui_labels.rs) stands in.
// - The function is `libelle`, not `t`: a global `t` would be shadowed by
//   the page's many local `t` variables, or clash with a top-level one.
//
// A classic script, not an ES module, on purpose: the page's modules
// (ui/*.js, T-435) read libelle() as a global, so the page still arms and
// stops with its fallback texts when this file does not load (an import
// that failed would stop every module: labels.spec.ts). The studio serves
// it itself (web.rs): no network. Tests: web.rs `ui_labels_*`, e2e
// labels.spec.ts.
const LABELS = Object.freeze({
  // Barre du haut
  'barre.produit': 'Laser Studio',
  'barre.maitre': 'Maître',
  'barre.plus': 'Autres commandes de tempo',
  'barre.plus.info': '÷2, ×2, décaler le temps, latence et pilote automatique',
  'barre.maitre.info': 'Luminosité maître : 100 % au plus, la limite de sécurité s\'applique après',
  'barre.wifi': 'Wi-Fi',
  'barre.sd_noir': 'SD : NOIR',
  'barre.batterie': 'Sur batterie{pct}',
  'barre.batterie.faible': 'Batterie faible{pct} : branchez le secteur',
  'barre.batterie.info': 'Le Mac n\'est pas sur secteur : branchez-le avant le show. À batterie critique, macOS le met en veille et le show s\'arrête. Le laser n\'est ni désarmé ni atténué pour autant.',
  'barre.chauffe': 'Le Mac chauffe : images en retard possibles',
  'barre.surchauffe': 'Le Mac surchauffe : images en retard',
  'barre.chauffe.info': 'macOS signale un état thermique élevé : le Mac ralentit. Aérez-le, sortez-le du soleil. Le laser n\'est ni désarmé ni atténué pour autant.',
  'barre.agenda': 'Agenda actif',
  'barre.agenda.info': 'Agenda actif : il lance des lectures à heure fixe (TIMELINE › Agenda). Il n\'arme jamais le laser.',
  'barre.secours': 'PRINCIPALE',
  'barre.secours.info': 'Machine de secours : clic = RÉGLAGES › Machine de secours',
  'barre.relais': 'Prendre le relais',
  'barre.relais.info': 'Cette machine devient la principale. Le laser reste désarmé : vous devrez l\'armer vous-même. Pas de raccourci clavier.',
  'barre.presence.info': 'Battement de l\'interface : sans aucune page ouverte, le laser se désarme',
  'barre.presence.absent': 'Opérateur absent',
  'barre.presence.present': 'Opérateur présent',
  'barre.moteur': 'Moteur',
  'barre.moteur.info': 'Chien de garde du moteur',
  'barre.moteur.bloque': 'Moteur bloqué — laser coupé',
  'barre.midi.info': 'Contrôleurs MIDI : clic = RÉGLAGES › Contrôleur',
  'barre.midi.attente': 'MIDI : …',
  'barre.dmx': 'DMX',
  'barre.dmx.info': 'Entrée DMX (Art-Net / sACN) : clic = RÉGLAGES › Entrées réseau',
  'barre.telecommande.aucune': 'Télécommande : 0',
  'barre.telecommande.info': 'Télécommande autorisée sur le réseau local : clic = RÉGLAGES › Télécommande',
  'barre.liaison_perdue': 'Connexion au moteur perdue',

  // Projet
  'projet.bouton': 'Projet ▾',
  'projet.bouton.info': 'Projet : Cmd+S enregistrer, Cmd+O ouvrir',
  'projet.nom.sans_titre': 'Sans titre',
  'projet.nom.info': 'Projet ouvert (• = modifications non enregistrées)',
  'projet.menu.nouveau': 'Nouveau projet…',
  'projet.menu.nouveau_modele': 'Nouveau depuis un modèle ▸',
  'projet.menu.enregistrer_modele': 'Enregistrer comme modèle…',
  'projet.menu.ouvrir': 'Ouvrir…',
  'projet.menu.recents': 'Récents',
  'projet.menu.enregistrer': 'Enregistrer',
  'projet.menu.enregistrer_sous': 'Enregistrer sous…',
  'projet.menu.enregistrer_version': 'Enregistrer une version…',
  'projet.menu.versions': 'Versions…',
  'projet.menu.paquet_exporter': 'Exporter un paquet…',
  'projet.menu.paquet_importer': 'Importer un paquet…',
  'projet.menu.import_partiel': 'Importer depuis un projet…',
  'projet.menu.a_propos': 'À propos de Laser Studio…',

  // Tempo
  'tempo.barre.info': 'Tempo : Entrée = tap, ⌫ = recaler sur le 1',
  'tempo.bpm.libelle': 'Tempo (BPM)',
  'tempo.tap.bouton': 'Tap',
  'tempo.tap.info': 'tempo.tap (Entrée)',
  'tempo.resync.bouton': 'Sync 1',
  'tempo.resync.info': 'tempo.resync (⌫)',
  'tempo.moitie.libelle': 'Tempo divisé par deux',
  'tempo.double.libelle': 'Tempo doublé',
  'tempo.retarder.libelle': 'Retarder la phase',
  'tempo.avancer.libelle': 'Avancer la phase',
  'tempo.auto.bouton': 'Auto',
  'tempo.auto.info': 'tempo.auto : le tempo suit la musique détectée (un tap reprend la main)',
  'tempo.guide.bouton': 'Guider',
  'tempo.guide.info': 'tempo.guide : tapez le tempo pour guider la détection, sans l\'imposer',
  'tempo.source.info': 'Tempo auto : état du suivi et confiance de la détection',
  'tempo.horloge_midi.info': 'Le tempo suit l\'horloge MIDI : Tap, Nudge et BPM sont ignorés (RÉGLAGES › Horloge MIDI)',
  'tempo.ableton_link.info': 'Le tempo suit Ableton Link : Nudge, Sync 1 et Tempo auto sont ignorés, le BPM est envoyé à la session (RÉGLAGES › Ableton Link)',
  'tempo.vitesses.info': 'Groupes de vitesse qui ne suivent pas le tempo à ×1, et la cadence des fondus (LIVE › Direct › Vitesses)',
  'tempo.temps1.suggere': 'Temps 1 suggéré',
  'tempo.temps1.info': 'Temps 1 suggéré par l\'analyse audio (voyant en pointillé). En Tempo auto, la mesure ne se recale seule que sur un drop ou quand la détection devient sûre.',
  'tempo.temps1.accepter': 'Accepter',
  'tempo.temps1.accepter.info': 'tempo.downbeat_accept : recaler la mesure sur le temps 1 suggéré',
  'tempo.phrase.info': 'Mesure dans la phrase (8 ou 16 mesures), comptée depuis le dernier drop ou la dernière fin de break',

  // Latence
  'latence.bouton': 'Lat.',
  'latence.bouton.info': 'Latence : décalage de sortie, latence estimée, calage à l\'oreille',
  'latence.decalage.libelle': 'Décalage de sortie (ms)',
  'latence.decalage.info': 'tempo.output_offset : les effets cadencés lisent le temps en avance d\'autant (+40 = 40 ms plus tôt)',
  'latence.estimee.attente': 'Latence estimée : …',
  'latence.oreille.bouton': 'Caler à l\'oreille',
  'latence.eclair.info': 'Éclair : le temps que le studio envoie au laser (aperçu)',
  'latence.oreille.aide': 'Un clic sonne sur chaque temps. Lancez un effet au temps (strobe cadencé) et réglez le décalage jusqu\'à ce que l\'éclair du laser tombe sur le clic. Le point montre ce que le studio envoie.',
  'latence.securite': 'Ne retarde jamais l\'arrêt : Échap, noir et désarmement agissent sur l\'image suivante.',

  // Pilote automatique
  'pilote.bouton': 'Pilote',
  'pilote.bouton.info': 'Pilote automatique : change de cue toutes les N mesures (coupé)',
  'pilote.titre': 'Pilote automatique',
  'pilote.activer': 'Activer',
  'pilote.activer.info': 'vlj.enabled : n\'arme jamais le laser ; l\'arrêt d\'urgence ou un désarmement le coupe',
  'pilote.changement': 'Changement',
  'pilote.chaque.1': 'Toutes les mesures',
  'pilote.chaque.2': 'Toutes les 2 mesures',
  'pilote.chaque.4': 'Toutes les 4 mesures',
  'pilote.chaque.8': 'Toutes les 8 mesures',
  'pilote.ordre': 'Ordre',
  'pilote.ordre.suite': 'Dans l\'ordre',
  'pilote.ordre.aleatoire': 'Aléatoire',
  'pilote.pages': 'Pages (aucune cochée = page affichée)',
  'pilote.public': 'Autoriser le contenu « public »',
  'pilote.public.info': 'Cues dont la case porte l\'étiquette « public » : jamais choisis sans cette case',
  'pilote.securite': 'Il ne fait que lancer des cues : n\'arme jamais, la sécurité s\'applique ensuite. Un cue lancé à la main reprend la main ; l\'arrêt d\'urgence ou un désarmement le coupe.',

  // Mode spectacle
  'spectacle.libelle': 'Mode spectacle',
  'spectacle.info': 'Mode spectacle : verrouiller l\'édition (cues, maîtres, tempo, noir et laser restent actifs)',
  'spectacle.verrouille': 'Verrouillé',
  'spectacle.verrouille.info': 'Mode spectacle actif : maintenir pour déverrouiller (1 s)',

  // Entrées externes
  'entrees.portail': 'Entrées externes',
  'entrees.portail.court': 'Ext.',
  'entrees.portail.info': 'Entrées externes fermées : DMX, OSC et timecode ne changent rien (sauf le noir et l\'arrêt d\'urgence) · Alt+X',
  'entrees.dmx.mvr': 'Exporter MVR…',
  'entrees.dmx.mvr.info': 'MVR 1.6 : le plan d\'implantation pour la console ou le visualiseur (grandMA3, Capture…) : un appareil par zone activée, à son adresse DMX et à la place de son projecteur dans le lieu, avec la fiche GDTF. Univers 0 : non patché.',
  'entrees.oscquery.activer': 'Publier la liste des adresses (OSCQuery, lecture seule)',
  'entrees.oscquery.port': 'Port OSCQuery (HTTP)',
  'entrees.oscquery.port.auto': 'port OSC + 1',
  'entrees.oscquery.annoncer': 'Annoncer sur le réseau (Bonjour _oscjson._tcp)',
  'entrees.oscquery.aide': 'Le contrôleur (TouchOSC, Chataigne, Open Stage Control…) lit les adresses /ls/…, leurs types et leurs plages à cette URL. Lecture seule : rien ne s\'écrit par là, aucune adresse n\'arme le laser, et les entrées externes décident toujours à la réception. Écoute sur l\'adresse de l\'entrée OSC, seulement quand elle est allumée. Sans l\'annonce, tapez l\'URL dans le contrôleur.',
  'entrees.oscquery.coupe': 'Coupé',
  'entrees.oscquery.sans_osc': 'En attente : allumez l\'entrée OSC',
  'entrees.oscquery.url': 'URL à saisir dans le contrôleur : {url} — {n} requête(s)',
  'entrees.oscquery.ouverture': 'Ouverture…',
  'entrees.erreur.delai': 'Le studio n\'a pas répondu à temps : réglage non confirmé, l\'affichage reprend l\'état du studio.',
  'entrees.erreur.injoignable': 'Studio injoignable : réglage non enregistré, l\'affichage reprend l\'état du studio.',

  // Sécurité: never empty (a test checks every securite.* key).
  'securite.noir': 'Noir',
  'securite.noir.info': 'Noir : coupe le laser tout de suite (comme Maj+Échap), toujours actif · Échap = ARRÊT (verrouillé)',
  'securite.arret': 'ARRÊT',
  'securite.arret.info': 'Arrêt d\'urgence (Échap) · safety.estop',
  'securite.ciel': 'CIEL COUPÉ',
  'securite.ciel.info': 'Ciel coupé (Alt+C) : éteint tout au-dessus de l\'horizon sans désarmer · safety.sky_off',
  'securite.laser.info': 'Espace : armer/désarmer · Maj+Échap : désarmer',
  'securite.laser.off': 'LASER OFF',
  'securite.laser.on': 'LASER ON',
  'securite.laser.arme': 'LASER ARMÉ',
  'securite.laser.moteur_inconnu': 'MOTEUR ?',
  'securite.laser.arret_materiel': 'Arrêt matériel à vérifier',
  'securite.laser.liste': 'Liste de contrôle à faire',
  'securite.desarme': 'Désarmé',
  'securite.desarme.raison': 'Désarmé — raison : {raison} ({source}), {heure}',
  'securite.arme.depuis': 'Armé depuis {duree} ({source})',
  'securite.estop': 'ARRÊT D\'URGENCE',
  'securite.maintien.actif': 'ÉMISSION — maintien actif',
  'securite.maintien.demande': 'MAINTENIR « {touche} » POUR ÉMETTRE',
  'securite.estop.banniere': 'ARRÊT D\'URGENCE — réinitialiser pour réarmer',
  'securite.estop.reinitialiser': 'Réinitialiser l\'arrêt d\'urgence',
  'securite.public': 'BALAYAGE PUBLIC',
  'securite.public.info': 'Balayage public déverrouillé : les zones Public reçoivent de la lumière atténuée. Le logiciel ne rend pas cela sûr ; il ne détecte pas une panne des galvos.',
  'securite.profil.info': 'Profil de sécurité actif (RÉGLAGES › Sécurité)',

  // Bandeaux sous la barre
  'bandeau.mire': 'Mire affichée — cues et timeline en pause',
  'bandeau.mire.quitter': 'Quitter la mire',
  'bandeau.zones': 'Vérification des zones',
  'bandeau.zones.quitter': 'Quitter',
  'bandeau.midi_perdu': 'Contrôleur MIDI déconnecté',
  'bandeau.dmx_perdu': 'Signal DMX perdu',
  'page.incomplete': 'Interface incomplète : rechargez la page',
  'bandeau.disque.fermer.info': 'Fermer ce bandeau (l\'avertissement reste dans la ligne d\'état)',

  // Espaces de travail et barre des onglets
  'espaces.libelle': 'Espaces de travail',
  'espaces.live': 'LIVE',
  'espaces.live.info': 'Jouer en direct (F1)',
  'espaces.timeline': 'TIMELINE',
  'espaces.timeline.info': 'Shows et timeline (F2)',
  'espaces.creation': 'CRÉATION',
  'espaces.creation.info': 'Créer du contenu (F3)',
  'espaces.reglages': 'RÉGLAGES',
  'espaces.reglages.info': 'Calibration, sécurité, MIDI, OSC/Art-Net (F4)',
  'espaces.pupitre': 'PUPITRE',
  'espaces.pupitre.info': 'Pupitre personnalisé : vos boutons, curseurs et pavés XY (Alt+F5)',
  'espaces.aide.laser': 'laser ·',
  'espaces.aide.reste': 'arrêt, dans tous les onglets · lettres = cues (LIVE)',
  'historique.libelle': 'Historique des modifications',
  'historique.annuler': 'Annuler',
  'historique.annuler.vide': 'Rien à annuler',
  'historique.retablir': 'Rétablir',
  'historique.retablir.vide': 'Rien à rétablir',
  'historique.bouton': 'Historique',
  'historique.bouton.info': 'Historique des modifications (cliquer une ligne annule jusqu\'à elle)',
  'fenetres.ouvrir': 'Ouvrir une fenêtre',
  'fenetres.ouvrir.info': 'Ouvrir une fenêtre : Sortie, Grille, Direct ou 3D, pour un deuxième écran ou un écran tactile. Chaque fenêtre montre l\'état du laser et Échap y reste l\'arrêt d\'urgence.',
  'fenetres.echap': 'Échap = arrêt d\'urgence',
  'fenetres.plein_ecran': 'Plein écran',
  'fenetres.plein_ecran.quitter': 'Quitter le plein écran',
  'fenetres.plein_ecran.info': 'Plein écran. Échap reste l\'arrêt d\'urgence : quitter le plein écran autrement que par ce bouton arrête aussi le laser.',
  'vues.bouton': 'Vues ▾',
  'vues.bouton.info': 'Vues enregistrées : rappeler l\'arrangement de l\'écran (Alt+F1 … F4, F6 … F9). La sortie ne change pas.',
  'recherche.bouton': 'Rechercher…',
  'recherche.bouton.info': 'Rechercher un cue, une page, une scène, un show, un contrôle, un réglage ou une page d\'aide (Cmd+K ou Cmd+F)',
  'commandes.bouton': 'Commandes',
  'commandes.bouton.info': 'Ligne de commande : régler un contrôle, lancer un cue, page, tempo, noir (Cmd+Maj+K). N\'arme jamais le laser.',
  'raccourcis.bouton': 'Raccourcis',
  'raccourcis.bouton.info': 'Tous les raccourcis clavier (?)',
  'aide.bouton': '? Aide',
  'aide.bouton.info': 'Aide intégrée, raccourcis et visite guidée',
  'notifications.libelle': 'Notifications',

  // Timecode sortant en LTC (T-501)
  'timeline.tco.ltc': 'LTC',
  'timeline.tco.ltc.info': 'Timecode LTC (SMPTE) sur une voie d\'une sortie audio, pour une console, un serveur vidéo ou un enregistreur. N\'arme jamais le laser.',
  'timeline.tco.ltc.sortie': 'Sortie audio',
  'timeline.tco.ltc.voie': 'Voie',
  'timeline.tco.ltc.niveau': 'Niveau (dBFS)',
  'timeline.tco.ltc.niveau.info': 'Niveau crête du signal LTC : -18 dBFS par défaut, de -40 à 0',
  'timeline.tco.ltc.aucune': 'Aucune sortie audio disponible (audio désactivé ?).',
  'timeline.tco.ltc.ouverte': 'LTC : {sortie}',

  // Aperçu : badge de sécurité de la vue 3D (T-512). La forme courte,
  // en vue étroite avec le panneau « Préparé », nomme toujours les faisceaux.
  'apercu.badge.public': 'Faisceaux dans le public',
  'apercu.badge.public.court': 'Faisc. public',
  // Commun
  'commun.fermer': 'Fermer',
  'commun.oui': 'Oui',
  'commun.non': 'Non',
  'commun.annuler': 'Annuler',
  'scripts.confiance.oui': 'Activer les scripts de ce projet',
  'scripts.confiance.non': 'Laisser désactivés',
});

// The text of `key`, with its {parameters} filled in (see the head of the file).
function libelle(key, params) {
  let k = key;
  if (params && typeof params.n === 'number' && !(k in LABELS) && (k + '.un') in LABELS) {
    k += Math.abs(params.n) < 2 ? '.un' : '.autre';
  }
  const v = LABELS[k];
  if (typeof v !== 'string') {
    console.warn('libellé inconnu : ' + key);
    return '⟦' + key + '⟧';
  }
  if (!params) return v;
  return v.replace(/\{([a-z_]+)\}/g, (m, name) => (name in params ? String(params[name]) : m));
}

// The attributes applyLabels() fills: data attribute → what it sets.
const LABEL_ATTRS = [['t-title', 'title'], ['t-placeholder', 'placeholder'], ['t-aria-label', 'aria-label']];

// Fill every data-t* of `root` (the page at start-up, or a fragment just
// built from markup) in one pass.
function applyLabels(root = document) {
  for (const el of root.querySelectorAll('[data-t], [data-t-title], [data-t-placeholder], [data-t-aria-label]')) {
    const d = el.dataset;
    if (d.t) el.textContent = libelle(d.t);
    for (const [attr, target] of LABEL_ATTRS) {
      const k = el.getAttribute('data-' + attr);
      if (k) el.setAttribute(target, libelle(k));
    }
  }
}
