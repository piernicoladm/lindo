# Lindo

Lindo 0.8.0 è una utility macOS scritta in Rust per individuare cosa occupa spazio e scegliere quali file conservare. Richiede macOS 26 e Apple Silicon. La panoramica mostra lo spazio libero, permette di cercare gli strumenti e di salvare i preferiti.

## Funzioni

- Apri la mappa proporzionale per esplorare una cartella. Salva la misura per confrontarla con una scansione successiva nella stessa sessione.
- Trova duplicati esatti, file grandi o vecchi, installatori, archivi e immagini JPEG, PNG o HEIC simili. Per le immagini puoi esaminare i percorsi e scegliere quali copie mettere nel Cestino.
- Esamina cache di sviluppo, modelli AI, backup iOS, app e relativi file riconosciuti. Puoi ripristinare le preferenze di un’app verificata o esaminare le revisioni di un documento locale. La revisione mostra gli elementi selezionati e la copia protetta di ogni gruppo.
- Gestisci le copie locali degli allegati di Apple Mail, i messaggi selezionati, i link di disiscrizione e le schede duplicate di Chrome. Su iCloud puoi liberare un download già sincronizzato.
- Apri Archivi cloud per consultare quota e cartelle remote di Dropbox, Google Drive e OneDrive tramite rclone. Spostamento in una cartella di revisione e rimozione online sono azioni distinte.
- Consulta volumi, snapshot Time Machine, risorse, avvii automatici e aggiornamenti. Gli strumenti di manutenzione richiedono conferma. Per le azioni privilegiate la richiesta di autorizzazione viene da macOS.
- Cerca malware con ClamAV o sorveglia una cartella mentre l’app è aperta, aggiorna le firme e rivedi le rilevazioni prima del Cestino. Monitora il disco dalla barra dei menu mentre Lindo è aperta.

Il dettaglio dei provider, dei limiti e delle verifiche è in [FEATURES.md](FEATURES.md).

## Provala dal sorgente

Servono un Mac Apple Silicon, macOS 26, Rust con Cargo e gli Apple Command Line Tools. Cargo scarica le dipendenze bloccate in Cargo.lock alla prima compilazione.

```sh
git clone https://github.com/piernicoladm/lindo.git
cd lindo
./build.sh --open
```

Puoi anche scegliere **Code → Download ZIP** nella pagina del repository, estrarre l'archivio e aprire il terminale nella cartella estratta prima di eseguire `./build.sh --open`.

Il comando prepara una build locale ad hoc e apre la copia in staging. Non sostituisce un'installazione esistente. La posizione della `.app` viene stampata nel terminale; i prodotti di build restano in `~/Library/Developer/Lindo/`.

Questa distribuzione contiene il sorgente. Una build locale non è una release notarizzata da Apple. Per compilazione, test, installazione e firma leggi [BUILD.md](BUILD.md).

## Prima di rimuovere qualcosa

Controlla sempre percorsi e copia conservata. Sulle cartelle sincronizzate, il Cestino può propagare la rimozione agli altri dispositivi e alle persone con cui condividi i file. Lo spazio recuperabile può differire dalla dimensione indicata, ad esempio con cloni APFS, compressione e hard link. Lindo mostra le scansioni interrotte o incomplete.

Il Cestino rimane un'operazione non atomica rispetto alle verifiche precedenti. File Provider, volumi esterni e filesystem di rete richiedono verifiche sul dispositivo: i test sintetici non dimostrano ogni possibile comportamento del sistema. Le protezioni e i limiti sono descritti in [SECURITY.md](SECURITY.md).

## Sviluppo

```sh
./tools/selftest.sh
./tools/test-release-safety.sh
python3 tools/test-export.py
```

La CI esegue test del motore, controlli dell'app con dati di esempio, regressioni degli script di rilascio ed esportazione del sorgente. Non pulisce dati personali e non pubblica automaticamente release. Vedi [CONTRIBUTING.md](CONTRIBUTING.md).

Licenza [MIT](LICENSE), © 2026 Piernicola De Maria.
