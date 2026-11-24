# Strumenti di Lindo 0.8

| Esigenza | Come usarla in Lindo | Ambito |
| --- | --- | --- |
| Analisi rapida | Panoramica, Analizza queste tre aree | Mappa, cache browser e log. Mostra percorsi, avanzamento, esclusioni ed errori. |
| Spazio su disco | Mappa dello spazio, Mappa proporzionale | Le aree rappresentano byte allocati. Clic su una cartella per approfondire. Misura di riferimento per cartella nella sessione. |
| File grandi, vecchi e download | File grandi oppure Download e immagini | Filtri per tipo, dimensione e data di modifica. Il Cestino richiede revisione. |
| Duplicati e foto simili | Duplicati oppure Download e immagini, Immagini simili | I duplicati sono confrontati byte per byte anche tra cartelle o volumi scelti insieme. La somiglianza percettiva è disponibile per JPEG, PNG, HEIC e HEIF. La copia di riferimento viene ricontrollata. |
| Cache, app e residui | Cache dei browser, Log, Applicazioni e Pulizia avanzata | Cache HTTP, log archiviati, app di terzi, componenti riconosciuti, cache utente attribuite e cache Xcode. Cache e log di sistema hanno un inventario consultabile. Preferenze danneggiate e avvii obsoleti si possono rivedere. Gli archivi degli utenti rimossi si possono ispezionare nel Finder. I residui di attribuzione incerta si ispezionano nel Finder. |
| Modelli AI, installatori e backup locali | Pulizia avanzata | Rimozione di modelli Hugging Face verificati e modelli Ollama tramite servizio locale con ricontrollo del digest. Identificazione di installatori PKG/DMG e backup iOS completi, anche archiviati. Il formato e l’identità vengono ricontrollati. |
| Ripristino app | Applicazioni, Ripristino app | Backup delle preferenze del dominio di una app di terzi verificata. Reset e ripristino richiedono conferma. Lingue e architetture presenti si possono ispezionare. |
| Revisioni documento | Mappa dello spazio, Revisioni documento | Selezione di un documento locale. Rimozione permanente della sola revisione storica identificata tramite macOS, con conferma separata. |
| Cestini e snapshot | Strumenti di sistema, Volumi | Finder per lo svuotamento definitivo. Rimozione selettiva degli snapshot locali di Time Machine. Il disco e gli snapshot usano le informazioni fornite da macOS. |
| Manutenzione e autorizzazioni | Strumenti di sistema | Cache DNS, rilascio cache RAM del disco, riduzione di snapshot locali, ripristino dei permessi della Home, indicizzazione Spotlight, elementi di login, attività in background, protezioni e permessi macOS. Verifica del filesystem in sola lettura, accesso a SOS, archiviazione e Time Machine. Alcuni controlli si completano in Impostazioni di Sistema, Utility Disco, Mail o Monitoraggio Attività. |
| Aggiornamenti | Strumenti di sistema e Aggiornamenti app | macOS e App Store tramite Apple. Homebrew verifica e aggiorna un pacchetto alla volta. Per le app con feed Sparkle, Lindo verifica la versione e apre l’app firmata per l’installazione con l’updater dell’editore. |
| Antivirus | Strumenti di sistema, Protezione | Motore e firme ClamAV opzionali. Scansione su richiesta e sorveglianza post-scrittura facoltativa di una cartella mentre Lindo è aperta. Limiti di 64 MiB per file, 2.000 file, 20.000 voci e 15 minuti per cartella. Conferma e nuova rilevazione prima del Cestino. |
| Posta e allegati | Mail e browser | Apple Mail, una casella scelta, navigabile in pagine di 100 messaggi. Revisione di messaggi e link HTTPS List-Unsubscribe. La rimozione delle copie in Mail Downloads conserva il messaggio. La rimozione dei messaggi selezionati interessa l’account. |
| Schede e privacy | Mail e browser | Chrome identifica schede web duplicate nella stessa finestra e ricontrolla quella da conservare. Cronologia e cookie si gestiscono nelle schermate native del browser. Reti note ed elementi recenti si gestiscono nei controlli macOS indicati. |
| File locali sincronizzati | Mail e browser, Cloud locale | iCloud rimuove la copia locale di un file già caricato. I client Dropbox, OneDrive e Google Drive espongono la loro azione per liberare spazio nel Finder. |
| File solo online | Archivi cloud | Dropbox, Google Drive e OneDrive tramite rclone. Quota, navigazione, filtro, spostamento sullo stesso account e rimozione presso il provider. Una cartella di revisione continua a consumare quota. |
| Monitor e schermo | Panoramica | Disco, carico medio, memoria, batteria e velocità di rete in barra dei menu mentre Lindo è aperta. Schermo nero per 45 secondi, blocco tastiera opzionale con autorizzazione macOS. Esc, clic e timer permettono di uscire. macOS può riservare i tasti hardware. |
| Accesso agli strumenti | Panoramica | Ricerca per nome o funzione, preferiti salvati sul Mac, sezioni raggiungibili dalla barra laterale. |

## Copertura e differenze

Lindo usa le API di macOS, i provider configurati e strumenti installabili. L’analisi rapida comprende tre aree. Le altre scansioni sono avviate dalla sezione scelta. L’antivirus usa le firme ClamAV. Il monitor osserva le scritture nella cartella scelta. Un file creato e rimosso fra due osservazioni può sfuggire. La gestione Mail richiede un account configurato in Apple Mail. Restano fuori dalla copertura il confronto di RAW e librerie Foto, la rimozione di lingue o architetture interne dai bundle firmati, il blocco preventivo dell’esecuzione sull’intero Mac e la classificazione cloud proprietaria di MacPaw. I modelli e i residui non attribuibili con precisione si possono ispezionare. Lo spazio recuperato effettivo dipende da APFS, Cestino e criteri di conservazione dei servizi.

Le verifiche usano fixture sintetiche. La finestra, le operazioni recuperabili tramite il Cestino, le firme ClamAV, le immagini HEIC e rclone locale hanno prove su macOS. Le mutazioni amministrative, gli account cloud, Mail e Chrome usano simulazioni o controlli di interfaccia. Il blocco globale della tastiera ha una prova simulata. Per la validazione su dispositivi e account reali servono autorizzazioni e dati di prova propri.

## Prove su macOS 26

- La mappa ha contato 120.120 voci in 306 ms e gestito 15.000 file nella stessa cartella in 86 ms. Il confronto di 3.600 duplicati ha restituito un gruppo in 553 ms. L’intervallo massimo tra frame durante queste scansioni su fixture in Applicazioni è stato 25 ms. Nel percorso Documenti il confronto ha richiesto 27-31 secondi.
- Su un volume sintetico HFS+ da 32 MiB che rispondeva ENOSPC, con 56 KiB ancora riportati come disponibili, la mappa è terminata senza errori.
- In finestra compatta e standard, Accessibility ha selezionato un file per nome su 12.061 file sintetici, aperto la revisione e annullato. A fine prova tutti i file erano ancora presenti.
- Sono stati verificati un JPEG e un HEIC della stessa foto, un duplicato realmente spostato nel Cestino e ripristinato, due revisioni native dello stesso documento e il mantenimento della revisione corrente.
- ClamAV ha scaricato le firme ufficiali in un database di prova. Un database separato con firma sintetica ha permesso di provare rilevazione, monitoraggio, Cestino e ripristino senza malware reale. Una verifica del filesystem è stata eseguita su un’immagine disco sintetica montata in sola lettura.
- La suite verifica inoltre con fixture il cambio di identità, i file aperti, i provider, l’annullamento e le conferme. I log della CI sono disponibili nella scheda Actions del repository.

## Riferimenti di prodotto

Il confronto delle funzioni usa la documentazione ufficiale delle edizioni CleanMyMac, compresi i servizi facoltativi. [Catalogo CleanMyMac](https://macpaw.com/support/cleanmymac/knowledgebase/my-tools) · [Differenze tra edizioni](https://macpaw.com/support/cleanmymac/knowledgebase/missing-features) · [Cloud Cleanup](https://macpaw.com/support/cleanmymac/knowledgebase/cloud-cleanup-start)

Per mappa, approfondimento delle cartelle e componenti associati a un’app. [DaisyDisk](https://daisydiskapp.com/manual/) · [GrandPerspective](https://grandperspectiv.sourceforge.net/) · [AppCleaner](https://freemacsoft.net/appcleaner/)

Per il comportamento dei servizi integrati. [ClamAV](https://docs.clamav.net/manual/Usage/Scanning.html) · [rclone lsjson](https://rclone.org/commands/rclone_lsjson/) · [Sparkle publishing](https://sparkle-project.org/documentation/publishing/)
