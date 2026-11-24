# Sicurezza

Lindo parte con i permessi dell’utente. Le scansioni locali restano sul dispositivo. Le funzioni opzionali contattano il provider selezionato tramite rclone, il feed dell’editore, Homebrew o il servizio firme ClamAV. Mail, browser e Finder possono richiedere il permesso Automazione. Le operazioni amministrative hanno un elenco di comandi predefiniti e delegano l’autorizzazione a macOS. Lindo non raccoglie password né telemetria. Il monitor facoltativo vive nel processo della finestra.

## Confini e protezioni

Le scansioni di pulizia partono da radici esplicite. Escludono collegamenti simbolici, placeholder cloud non locali, pacchetti documento, credenziali e aree protette. La mappa dello spazio ha una policy distinta: può leggere metadati in Library e dentro pacchetti, ma non produce candidati alla rimozione e non legge contenuti dei documenti.

Le rimozioni richiedono selezione, revisione e conferma. Prima del Cestino vengono ricontrollati identità, scope e condizioni specifiche del candidato. I duplicati sono confrontati per hash e byte; una copia conservata resta protetta. Per cache e applicazioni, processi attivi o inventario incompleto bloccano la rimozione pertinente.

Sono presenti esclusioni conservative per nome, incluse alcune cartelle di archivi privati. Non costituiscono un controllo di accesso generale: non usare i filtri dell'app come sostituto dei permessi del filesystem.

## Modifiche ai bundle

L’alleggerimento usa una copia privata sullo stesso volume. Ricontrolla bundle, firma e identità del codice conservato prima di proporre lo scambio. La sostituzione conserva l’originale come backup. Finché il backup o la sua copia nel Cestino occupano il volume, i byte dell’originale restano allocati. I bundle che richiederebbero una nuova firma vengono esclusi. Lo spazio necessario per la copia viene verificato prima di iniziare.

## Operazioni online e irreversibili

Svuota Cestino chiede una conferma separata e usa Finder. Lo svuotamento può interessare più volumi montati. La rimozione di uno snapshot locale perde quel punto di ripristino. Per Mail e i provider cloud una modifica può sincronizzarsi con altri dispositivi. Archivi cloud rivalida identità e metadati del singolo file immediatamente prima della richiesta al provider. Conservazione, quota e tempi di ripristino dipendono dal servizio. Nel Finder “Rimuovi download” riguarda la copia locale.

## Limiti dichiarati

Le verifiche e `FileManager.trashItem` non sono una singola operazione atomica. Un processo esterno può modificare il filesystem fra il controllo finale e il Cestino. Non viene promesso isolamento da un processo ostile che opera con gli stessi privilegi dell'utente.

Le eliminazioni su cartelle sincronizzate possono propagarsi. I test sintetici non certificano ogni provider cloud o volume di rete. Una chiamata di sistema già bloccata può trattenere un worker fino al ritorno del sistema. Il rilevamento ClamAV richiede un motore installato e firme disponibili. Un esito pulito riguarda soltanto i file effettivamente analizzati. Firma e notarizzazione non dimostrano correttezza funzionale né assenza di ogni vulnerabilità.

## Segnalazioni

Per problemi senza informazioni sensibili, apri una issue indicando versione, macOS, passaggi e risultato atteso. Usa fixture sintetiche e rimuovi nomi di file personali dai log. Per vulnerabilità che renderebbero rischiosa la divulgazione immediata, usa la funzione GitHub **Security → Report a vulnerability** quando disponibile; non pubblicare dati personali, credenziali o exploit contro sistemi di terzi.

Le verifiche di sicurezza devono usare solo dati sintetici o un perimetro espressamente autorizzato. Il repository include regressioni avversariali, ma non presenta quei test come una certificazione di sicurezza indipendente.
