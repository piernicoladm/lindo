# Strumenti di Lindo 0.8

| Esigenza | Come usarla in Lindo | Ambito |
| --- | --- | --- |
| Analisi rapida | Panoramica, Analizza queste tre aree | Mappa, cache browser e log. Mostra percorsi, avanzamento, esclusioni ed errori. |
| Spazio su disco | Mappa dello spazio, Mappa proporzionale | Le aree rappresentano byte allocati. Clic su una cartella per approfondire. Misura di riferimento per cartella nella sessione. |
| File grandi, vecchi e download | File grandi oppure Download e immagini | Filtri per tipo, dimensione e data di modifica. Il Cestino richiede revisione. |
| Duplicati e foto simili | Duplicati oppure Download e immagini, Immagini simili | I duplicati sono confrontati byte per byte. La somiglianza percettiva è disponibile per JPEG e PNG. La copia di riferimento viene ricontrollata. |
| Cache, app e residui | Cache dei browser, Log, Applicazioni e Pulizia avanzata | Cache HTTP, log archiviati, app di terzi, componenti riconosciuti e cache Xcode. I residui di attribuzione incerta si ispezionano nel Finder. |
| Modelli AI, installatori e backup locali | Pulizia avanzata | Identificazione di modelli Hugging Face, installatori PKG/DMG e backup iOS completi, anche archiviati. Il formato e l’identità vengono ricontrollati. |
| Cestini e snapshot | Strumenti di sistema, Volumi | Finder per lo svuotamento definitivo. Rimozione selettiva degli snapshot locali di Time Machine. Il calcolo dello spazio tiene conto della disponibilità riportata dal sistema. |
| Manutenzione e autorizzazioni | Strumenti di sistema | Cache DNS, indicizzazione Spotlight, elementi di login, attività in background, protezioni e permessi macOS. Alcuni controlli si completano in Impostazioni di Sistema. |
| Aggiornamenti | Strumenti di sistema e Aggiornamenti app | macOS e App Store tramite Apple. Homebrew verifica e aggiorna un pacchetto alla volta. Per le app con feed Sparkle, Lindo verifica la versione e apre l’app firmata per l’installazione con l’updater dell’editore. |
| Antivirus | Strumenti di sistema, Protezione | Motore e firme ClamAV opzionali. Scansione su richiesta, file o cartella. Limiti di 64 MiB per file, 2.000 file, 20.000 voci e 15 minuti per cartella. Conferma e nuova rilevazione prima del Cestino. |
| Posta e allegati | Mail e browser | Apple Mail, una casella scelta e i primi 100 messaggi. Revisione di messaggi e link HTTPS List-Unsubscribe. La rimozione delle copie in Mail Downloads conserva il messaggio. La rimozione dei messaggi selezionati interessa l’account. |
| Schede e privacy | Mail e browser | Chrome identifica schede web duplicate nella stessa finestra e ricontrolla quella da conservare. Cronologia e cookie si gestiscono nelle schermate native del browser. |
| File locali sincronizzati | Mail e browser, Cloud locale | iCloud rimuove la copia locale di un file già caricato. I client Dropbox, OneDrive e Google Drive espongono la loro azione per liberare spazio nel Finder. |
| File solo online | Archivi cloud | Dropbox, Google Drive e OneDrive tramite rclone. Quota, navigazione, filtro, spostamento sullo stesso account e rimozione presso il provider. Una cartella di revisione continua a consumare quota. |
| Monitor e schermo | Panoramica | Spazio e carico medio in barra dei menu mentre Lindo è aperta. Schermo nero per 45 secondi con uscita via Esc. Le scorciatoie di sistema restano disponibili. |
| Accesso agli strumenti | Panoramica | Ricerca per nome o funzione, preferiti salvati sul Mac, sezioni raggiungibili dalla barra laterale. |

## Copertura e differenze

Lindo usa le API di macOS, i provider configurati e strumenti installabili. L’analisi rapida comprende tre aree. Le altre scansioni sono avviate dalla sezione scelta. L’antivirus è su richiesta, con firme ClamAV. La gestione Mail richiede un account configurato in Apple Mail. Le foto HEIC, RAW e le librerie di Foto, la rimozione di lingue o architetture dai bundle firmati, la cancellazione della cronologia Wi-Fi e la protezione antivirus in tempo reale restano fuori dalla copertura. I modelli e i residui non attribuibili con precisione si possono ispezionare. Lo spazio recuperato effettivo dipende da APFS, Cestino e criteri di conservazione dei servizi.

Le verifiche automatiche usano file e account sintetici. I test dei provider remoti, dei permessi TCC, delle operazioni amministrative e della cancellazione di dati personali non costituiscono una prova end-to-end su un account o un disco di produzione.

## Riferimenti di prodotto

Il confronto delle funzioni usa la documentazione ufficiale delle edizioni CleanMyMac, compresi i servizi facoltativi. [Catalogo CleanMyMac](https://macpaw.com/support/cleanmymac/knowledgebase/my-tools) · [Differenze tra edizioni](https://macpaw.com/support/cleanmymac/knowledgebase/missing-features) · [Cloud Cleanup](https://macpaw.com/support/cleanmymac/knowledgebase/cloud-cleanup-start)

Per mappa, approfondimento delle cartelle e componenti associati a un’app. [DaisyDisk](https://daisydiskapp.com/manual/) · [GrandPerspective](https://grandperspectiv.sourceforge.net/) · [AppCleaner](https://freemacsoft.net/appcleaner/)

Per il comportamento dei servizi integrati. [ClamAV](https://docs.clamav.net/manual/Usage/Scanning.html) · [rclone lsjson](https://rclone.org/commands/rclone_lsjson/) · [Sparkle publishing](https://sparkle-project.org/documentation/publishing/)
