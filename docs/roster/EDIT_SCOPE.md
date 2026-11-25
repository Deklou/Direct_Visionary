# Ciblage des modifications

Une modification propre à un personnage doit être liée à une identité stable (`RosterKey`), un combattant moteur et une liste explicite de costumes autorisés. Le nom de l'animation ou le `fighter_kind` seul ne suffit pas à isoler deux personnages qui partagent le même combattant moteur.

Le personnage d'origine utilise uniquement les costumes relevés dans les données **vanilla**. Un personnage moddé créé dans Visionary utilise les costumes déclarés par sa `SlotClone` (`AuthoredEntry::all_slots`). Le même réglage peut s'appliquer à toutes ses tenues sans duplication.

Le résolveur `src/roster/edit_scope.rs` refuse les cibles inconnues, les slots absents ou dupliqués, les conflits d'occupation, les données vanilla manquantes et les mods importés dont la cible n'est pas explicitement définie. Un costume inconnu à l'exécution ne correspond jamais à une règle.

La liaison enregistrée avec une modification doit être identique à la liaison résolue au moment de l'application. Une liaison manquante (ancien projet) ou différente (mod déplacé de c08 à c16) doit rester inactive tant qu'elle n'a pas été validée, sans effacer les données de l'utilisateur.

**État actuel :** le résolveur est disponible mais n'est pas encore branché à l'éditeur ni au plugin live. L'export ACMD protège les catégories `game_`, `effect_`, `sound_` et `expression_` quand le mouvement dispose d'une portée par costume et de ses scripts d'origine ; il refuse les catégories secondaires ambiguës ou sans source d'origine. L'édition de deux personnages partageant une même attaque sur un même combattant moteur exige encore un journal de modifications séparé par identité logique. Les anciens projets et le comportement live restent inchangés. Avant d'activer le live, le protocole devra vérifier que le plugin traite les costumes. Les paramètres natifs communs à tous les costumes ne sont pas isolables. Un mod `.nro` compilé demande une attribution explicite et ne peut pas être certifié par sa seule arborescence.
