import { Icon } from "./ui.tsx";

// First-run screen shown while no library is open.
export default function Onboarding({ actionsDisabled, createLibrary, openLibrary }: {
  actionsDisabled: boolean;
  createLibrary: (chooseLocation: boolean) => void;
  openLibrary: () => void;
}) {
  return (
    <>
      <main className="onboarding">
        <section className="welcome">
          <p className="eyebrow">
            <span />
            DEIN LOKALER ASSET-FUNDUS
          </p>
          <h1>
            Von der Sammlung
            <br />
            ins nächste <em>Spiel.</em>
          </h1>
          <p className="intro">
            Importiere Ordner oder ZIP-Pakete. Finde passende Sounds,
            Sprites und 3D-Modelle und nimm sie direkt in dein Projekt mit.
          </p>
          <div className="workflow" aria-label="Dein Arbeitsablauf">
            <div>
              <span>01</span>
              <strong>Pakete sammeln</strong>
              <p>Originalordner und Begleitdateien bleiben zusammen.</p>
            </div>
            <div>
              <span>02</span>
              <strong>Assets finden</strong>
              <p>Paket, Herkunft und Dateiname im Blick.</p>
            </div>
            <div>
              <span>03</span>
              <strong>Im Spiel verwenden</strong>
              <p>Originaldatei ins eigene Projekt exportieren.</p>
            </div>
          </div>
        </section>
        <section className="setup-card" aria-labelledby="setup-title">
          <div className="setup-illustration" aria-hidden="true">
            <div className="folder-back" />
            <div className="folder-front">
              <span className="sound-bars">
                <i />
                <i />
                <i />
                <i />
                <i />
                <i />
                <i />
              </span>
            </div>
            <span className="illustration-dot" />
          </div>
          <p className="eyebrow">ERSTER SCHRITT</p>
          <h2 id="setup-title">Deine eigene Bibliothek</h2>
          <p>
            Dein Katalog und deine importierten Pakete werden gemeinsam an
            einem Ort gespeichert.
          </p>
          <button
            className="button primary full"
            onClick={() => createLibrary(false)}
            disabled={actionsDisabled}
          >
            Standardbibliothek anlegen
            <Icon name="arrow" />
          </button>
          <p className="storage-hint">
            Im Benutzerordner unter „Game Asset Library“
          </p>
          <button
            className="button outlined full"
            onClick={() => createLibrary(true)}
            disabled={actionsDisabled}
          >
            <Icon name="folder" />
            Eigenen Speicherort wählen
          </button>
          <div className="setup-open">
            <span>Schon eine Bibliothek vorhanden?</span>
            <button
              className="text-button"
              onClick={openLibrary}
              disabled={actionsDisabled}
            >
              Bibliothek öffnen
              <Icon name="arrow" />
            </button>
          </div>
        </section>
      </main>
      <footer className="onboarding-footer">
        <span>Deine Bibliothek bleibt bei dir.</span>
        <span>
          Pakete importieren · Assets ansehen · Originale exportieren
        </span>
      </footer>
    </>
  );
}
