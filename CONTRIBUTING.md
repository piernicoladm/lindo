# Contribuire a Lindo

Apri una issue per descrivere un bug o una proposta, oppure una pull request con una modifica circoscritta. Mantieni codice e commenti tecnici in inglese e testi dell'interfaccia in italiano.

Prima di proporre una modifica:

1. Riproduci il problema con dati sintetici.
2. Mantieni prodotti di build e fixture fuori dal checkout.
3. Esegui `tools/selftest.sh`, `tools/test-release-safety.sh`, `cargo fmt --check` e `python3 tools/test-export.py`.
4. Descrivi trigger, comportamento prima/dopo, verifiche eseguite e limiti.

Per modifiche alla rimozione, includi una regressione che fallisca senza la protezione. Preserva selezione esplicita, revisione immutabile, copia conservata nei duplicati, controlli di identità/scope e il Cestino. Non aggiungere cancellazione permanente, helper root, scansioni automatiche o raccolta dati.

Non provare la pulizia su documenti personali. Le prove con il Cestino di macOS devono usare esclusivamente fixture create dal test e verificare il ripristino. Le prove manuali su cloud, volumi esterni, accessibilità e UI vanno dichiarate separatamente dai test automatici.

Non includere credenziali, certificati, dati clienti, log personali o cartelle `.app` nelle pull request. Gli strumenti di rilascio richiedono configurazione locale: non esiste un'identità di firma predefinita nel sorgente pubblico.

La CI ha permessi di sola lettura sul repository ed esegue codice delle pull request senza segreti. Non effettua upload ad Apple, installazioni sul Mac di un utente o pubblicazioni di release.

La sorgente del workflow è `tools/ci/public-ci.yml`; l'export la copia in `.github/workflows/ci.yml`. Mantieni i due file identici quando cambi la CI. `SOURCE-MANIFEST.json` descrive lo snapshot esportato: dopo modifiche ai file, rigenera l'export prima di distribuire un nuovo ZIP o aggiornare il manifest. Il manifest non certifica modifiche successive e non va aggiornato a mano.

I contributi sono distribuiti con la licenza MIT del repository.
