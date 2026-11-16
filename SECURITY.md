# Sicurezza

Lindo opera con i permessi dell'utente, senza helper privilegiati. Non ha rete applicativa, account, telemetria, attività in background o cancellazione permanente. Gli script di notarizzazione contattano Apple soltanto quando invocati esplicitamente dal distributore.

## Confini e protezioni

Le scansioni di pulizia partono da radici esplicite. Escludono collegamenti simbolici, placeholder cloud non locali, pacchetti documento, credenziali e aree protette. La mappa dello spazio ha una policy distinta: può leggere metadati in Library e dentro pacchetti, ma non produce candidati alla rimozione e non legge contenuti dei documenti.

Le rimozioni richiedono selezione, revisione e conferma. Prima del Cestino vengono ricontrollati identità, scope e condizioni specifiche del candidato. I duplicati sono confrontati per hash e byte; una copia conservata resta protetta. Per cache e applicazioni, processi attivi o inventario incompleto bloccano la rimozione pertinente.

Sono presenti esclusioni conservative per nome, incluse alcune cartelle di archivi privati. Non costituiscono un controllo di accesso generale: non usare i filtri dell'app come sostituto dei permessi del filesystem.

## Limiti dichiarati

Le verifiche e `FileManager.trashItem` non sono una singola operazione atomica. Un processo esterno può modificare il filesystem fra il controllo finale e il Cestino. Non viene promesso isolamento da un processo ostile che opera con gli stessi privilegi dell'utente.

Le eliminazioni su cartelle sincronizzate possono propagarsi. I test sintetici non certificano ogni provider cloud o volume di rete. Una chiamata di sistema già bloccata può trattenere un worker fino al ritorno del sistema. Non è presente antivirus o rilevamento malware. Firma e notarizzazione non dimostrano correttezza funzionale né assenza di ogni vulnerabilità.

## Segnalazioni

Per problemi senza informazioni sensibili, apri una issue indicando versione, macOS, passaggi e risultato atteso. Usa fixture sintetiche e rimuovi nomi di file personali dai log. Per vulnerabilità che renderebbero rischiosa la divulgazione immediata, usa la funzione GitHub **Security → Report a vulnerability** quando disponibile; non pubblicare dati personali, credenziali o exploit contro sistemi di terzi.

Le verifiche di sicurezza devono usare solo dati sintetici o un perimetro espressamente autorizzato. Il repository include regressioni avversariali, ma non presenta quei test come una certificazione di sicurezza indipendente.
