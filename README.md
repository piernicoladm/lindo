# Lindo

Una piccola utility nativa per capire cosa occupa spazio sul Mac e scegliere cosa spostare nel Cestino. Gratuita e open source, senza account, telemetria o servizi in background. Interfaccia in italiano.

**Stato: candidata 0.5.4, distribuzione del sorgente.** Le build di sviluppo non sono notarizzate. La CI pubblica documenta i controlli eseguiti; non sostituisce le prove sui dispositivi e sui servizi cloud reali.

## Cosa puoi fare

- Esplorare una mappa dello spazio in sola lettura.
- Trovare file grandi nelle cartelle che scegli.
- Confrontare duplicati esatti e scegliere la copia da conservare.
- Esaminare cache HTTP riconosciute e log utente vecchi di almeno sette giorni.
- Inventariare le applicazioni e rimuovere il solo bundle, mantenendo preferenze e documenti.

Nessun elemento viene preselezionato. Prima del Cestino vedi l'elenco preciso e le copie conservate. Lindo ricontrolla scope, identità dei file e le condizioni pertinenti alla rimozione. Non svuota il Cestino e non chiede privilegi amministratore.

## Provala dal sorgente

Servono un Mac Apple Silicon, macOS 26 e Swift 6.2 o successivo. Non ci sono dipendenze esterne. Con Apple Command Line Tools oppure Xcode installato:

```sh
./build.sh --open
```

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
