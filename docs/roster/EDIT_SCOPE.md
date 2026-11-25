# Ciblage des modifications

Une modification propre à un personnage doit être liée à une identité stable (`RosterKey`), un combattant moteur et une liste explicite de costumes autorisés. Le nom de l'animation ou le `fighter_kind` seul ne suffit pas à isoler deux personnages qui partagent le même combattant moteur.

Le personnage d'origine utilise uniquement les costumes relevés dans les données **vanilla**. Un personnage moddé créé dans Visionary utilise les costumes déclarés par sa `SlotClone` (`AuthoredEntry::all_slots`). Le même réglage peut s'appliquer à toutes ses tenues sans duplication.

Le résolveur `src/roster/edit_scope.rs` refuse les cibles inconnues, les slots absents ou dupliqués, les conflits d'occupation, les données vanilla manquantes et les mods importés dont la cible n'est pas explicitement définie. Un costume inconnu à l'exécution ne correspond jamais à une règle.

La liaison enregistrée avec une modification doit être identique à la liaison résolue au moment de l'application. Une liaison manquante (ancien projet) ou différente (mod déplacé de c08 à c16) doit rester inactive tant qu'elle n'a pas été validée, sans effacer les données de l'utilisateur.

**État actuel :** le résolveur et ses tests sont disponibles, mais aucun éditeur, message TCP, hook Skyline ou export ne les utilise encore. Les anciens projets et le comportement live restent inchangés. Les prochaines étapes devront brancher le contrôle aux trois destinations (éditeur, live, export) et prévoir un protocole vérifiant que le plugin sait traiter les costumes avant tout envoi. Les paramètres natifs communs à tous les costumes ne peuvent pas être présentés comme isolables. Un mod importé avec `.nro` compilé nécessite une attribution explicite et ne peut pas être certifié par simple lecture de ses fichiers.
