# Compilazione e verifica

Lindo richiede macOS 26, Apple Silicon e la toolchain stabile Rust con Cargo. Cargo usa le dipendenze registrate in Cargo.lock. Apple Command Line Tools è sufficiente per lo sviluppo locale; la CI usa Xcode.

```sh
rustc --version
cargo --version
./build.sh
./build.sh --debug --open
```

Il bundle viene preparato in `~/Library/Developer/Lindo/staging/`. Gli altri prodotti sono conservati sotto `~/Library/Developer/Lindo/`, fuori dal checkout. `--open` apre la copia preparata. `--no-install` è accettato per compatibilità ed equivale al comportamento predefinito.

Per sostituire esplicitamente `~/Applications/Lindo.app`, chiudi Lindo e usa `./build.sh --install`. La copia precedente viene conservata in `~/Library/Developer/Lindo/backups/`. Una build ad hoc valida può essere aggiornata normalmente; per sostituire una copia notarizzata, Developer ID o la cui identità non è verificabile serve anche `--replace-notarized`. Questo produce una copia di sviluppo senza notarizzazione.

## Test

```sh
./tools/selftest.sh --all-features
./tools/test-release-safety.sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
python3 tools/test-export.py
```

`selftest.sh` esegue le regressioni Rust su fixture. Il comando `Lindo --selftest` esegue una scansione sintetica dal binario del bundle. I controlli di rilascio sostituiscono firma, installazione e servizi Apple con stub.

Per verificare la finestra su 120.000 file in sottocartelle, 15.000 file nella stessa cartella e 3.600 duplicati, compila con `cargo build --locked --release --features ui-probe` e passa il binario e una directory locale di test esistente a `python3 tools/scale-ui.py`. La directory deve essere nel tuo utente e fuori da Library o Dropbox. La prova crea e rimuove le proprie fixture e richiede una sessione grafica macOS.

Per una verifica di aspetto senza cambiare le impostazioni del Mac, il binario del bundle accetta `-AppleInterfaceStyle Dark` oppure `-AppleInterfaceStyle Light`. L’avvio normale segue l’aspetto di sistema.

## Firma e notarizzazione per distributori

La firma è facoltativa per compilare e provare localmente. Per una distribuzione Developer ID, configura il tuo certificato nel Portachiavi e indica esplicitamente identità e team:

```sh
export LINDO_SIGNING_IDENTITY='Developer ID Application: Your Name (YOURTEAMID)'
export LINDO_SIGNING_TEAM='YOURTEAMID'
./build.sh --signed --no-install
```

Il team deve contenere dieci lettere maiuscole o cifre. Non mettere password, chiavi private o credenziali nel repository. Configura un profilo Keychain per `notarytool` con gli strumenti Apple, quindi usa il percorso del bundle stampato dalla build:

```sh
./tools/notarize.sh /path/to/Lindo.app --profile YOUR_PROFILE --prepare-only
./tools/notarize.sh /path/to/Lindo.app --profile YOUR_PROFILE
```

La preparazione crea un manifest SHA-256 senza effettuare upload. La notarizzazione conserva la submission e si riprende con lo stesso comando: exit 3 significa elaborazione pending o stato non disponibile. In caso di esito di upload incerto, lo script blocca un secondo invio; recupera l'UUID dai log e dalla cronologia Apple prima di riprendere. Non eliminare automaticamente i marker di incertezza.

Solo dopo stato Accepted, stapling e verifiche Gatekeeper, `ready.json` identifica il bundle e lo ZIP pronti. `LINDO_SIGNING_TEAM` è richiesto anche da `tools/install-notarized.sh /path/to/Lindo.app`. L'installer verifica il team scelto, il ticket, Gatekeeper e l'eventuale ricevuta; conserva la vecchia app e rifiuta downgrade senza `LINDO_ALLOW_DOWNGRADE=1`.

## Sorgente distribuibile

```sh
python3 tools/export-source.py --output /tmp/Lindo-source --zip /tmp/Lindo-source.zip
```

La destinazione deve essere nuova. L'export usa una allowlist, rifiuta symlink e possibili segreti e scrive `SOURCE-MANIFEST.json` con SHA-256 e permessi dei file. Lo ZIP usa ordine, timestamp e permessi normalizzati: a parità di sorgenti produce gli stessi byte. Non include storia Git, dati di lavoro, archivi di release o credenziali.

La CI usa il runner `macos-26` arm64 documentato da [GitHub](https://docs.github.com/en/actions/reference/runners/github-hosted-runners), con Xcode 26.0.1 e Xcode 26.6 per verificare due configurazioni SDK. Versioni e disponibilità sono verificate all'avvio. Riferimenti: [immagine runner](https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md) e [note Apple Xcode 26.0.1](https://developer.apple.com/documentation/xcode-release-notes/xcode-26_0_1-release-notes).

## Strumenti opzionali

Con Homebrew puoi eseguire `brew install clamav rclone`. In Lindo scegli Strumenti di sistema, Protezione e aggiornamento delle firme. Il primo download richiede una connessione. Un account Dropbox, Google Drive o OneDrive configurato con `rclone config` abilita Archivi cloud. Lindo non riceve password né token tramite la finestra. Apple Mail, Chrome e Finder gestiscono i permessi Automazione tramite macOS. La presenza dell'entitlement Apple Events rende possibile presentare la richiesta.
