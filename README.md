# Lindo

Lindo mostra quali file e cartelle occupano più spazio sul Mac e trova i duplicati esatti confrontando i file byte per byte. La scansione avviene sul Mac, nelle cartelle che scegli. Per ogni gruppo di duplicati puoi confrontare i percorsi, scegliere la copia da conservare e rivedere le altre prima di spostarle nel Cestino.

Lindo 0.9.4 richiede **macOS 26 o successivo su Apple Silicon**. Il codice è scritto in Rust e distribuito con [licenza MIT](LICENSE).

## Avvio rapido dal sorgente

Installa la toolchain stabile di Rust con Cargo e gli strumenti da riga di comando di Xcode. Poi apri il Terminale ed esegui questi comandi.

```sh
git clone https://github.com/piernicoladm/lindo.git
cd lindo
./build.sh --open
```

Puoi anche scaricare il sorgente da GitHub con **Code → Download ZIP**, estrarlo ed eseguire `./build.sh --open` nella cartella estratta. Alla prima compilazione Cargo scarica le dipendenze elencate in `Cargo.lock`.

Lo script apre la copia compilata in `~/Library/Developer/Lindo/staging/` e stampa il percorso della `.app`. Questa build locale usa una firma ad hoc ed è priva di notarizzazione Apple. Per installazione, test e firma consulta [BUILD.md](BUILD.md).

### Prima scansione

1. Apri **Mappa dello spazio** e scegli una cartella. Nella mappa proporzionale, i riquadri più grandi indicano gli elementi che occupano più spazio. Fai clic su una cartella per esplorarla.
2. Per confrontare copie anche in cartelle diverse, apri **Duplicati** e seleziona le cartelle da esaminare. Puoi interrompere la scansione mentre è in corso.
3. Apri un gruppo di risultati e scegli la copia da conservare. Controlla i percorsi delle copie selezionate e quella che rimarrà sul Mac.
4. In **File grandi**, seleziona le righe dei file da rivedere. Con due file compare **Rivedi 2 file per il Cestino**. Nella revisione puoi scegliere **Indietro** per cambiare la selezione oppure **Sposta nel Cestino** per confermare.

## Funzioni

- Apri la mappa proporzionale per esplorare una cartella. Salva la misura per confrontarla con una scansione successiva nella stessa sessione.
- Trova duplicati esatti, file grandi o vecchi, installatori, archivi e immagini JPEG, PNG o HEIC simili. Per le immagini puoi esaminare i percorsi e scegliere quali copie mettere nel Cestino.
- Esamina cache di sviluppo, modelli AI, backup iOS, app e relativi file riconosciuti. Per app universali idonee puoi creare una copia più leggera mantenendo la firma dell’editore. Puoi ripristinare le preferenze di un’app verificata o esaminare le revisioni di un documento locale. La revisione mostra gli elementi selezionati e la copia protetta di ogni gruppo.
- Gestisci le copie locali degli allegati di Apple Mail, i messaggi selezionati, i link di disiscrizione e le schede duplicate di Chrome. Su iCloud puoi liberare un download già sincronizzato.
- Apri Archivi cloud per consultare quota e cartelle remote di Dropbox, Google Drive e OneDrive tramite rclone. Spostamento in una cartella di revisione e rimozione online sono azioni distinte.
- Consulta volumi, snapshot Time Machine, risorse, avvii automatici e aggiornamenti. Gli strumenti di manutenzione richiedono conferma. Per le azioni privilegiate la richiesta di autorizzazione viene da macOS.
- Cerca malware con ClamAV o sorveglia una cartella mentre l’app è aperta, aggiorna le firme e rivedi le rilevazioni prima del Cestino. Monitora il disco dalla barra dei menu mentre Lindo è aperta.

Il dettaglio dei provider, dei limiti e delle verifiche è in [FEATURES.md](FEATURES.md).

## Prima di rimuovere qualcosa

Controlla sempre percorsi e copia conservata. Sulle cartelle sincronizzate, il Cestino può propagare la rimozione agli altri dispositivi e alle persone con cui condividi i file. La revisione mostra la dimensione selezionata e, per i file, una stima basata sui blocchi allocati dopo lo svuotamento. Un file con più hard link contribuisce alla stima una sola volta, quando sono selezionati tutti i suoi nomi. Cloni APFS e snapshot possono ridurre lo spazio effettivamente liberato. Per le selezioni che comprendono cartelle la stima complessiva è indicata come indisponibile. Lindo mostra le scansioni interrotte o incomplete.

Il Cestino rimane un'operazione non atomica rispetto alle verifiche precedenti. File Provider, volumi esterni e filesystem di rete richiedono verifiche sul dispositivo: i test sintetici non dimostrano ogni possibile comportamento del sistema. Le protezioni e i limiti sono descritti in [SECURITY.md](SECURITY.md).

## Sviluppo

```sh
./tools/selftest.sh
./tools/test-release-safety.sh
python3 tools/test-export.py
```

La CI esegue test del motore, controlli dell'app con dati di esempio, regressioni degli script di rilascio ed esportazione del sorgente. Non pulisce dati personali e non pubblica automaticamente release. Vedi [CONTRIBUTING.md](CONTRIBUTING.md).

Licenza [MIT](LICENSE), © 2026 Piernicola De Maria.
