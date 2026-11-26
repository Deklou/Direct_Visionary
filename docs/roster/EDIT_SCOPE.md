# Ciblage des modifications

Une modification est associée à une identité logique stable (`RosterKey`), un combattant moteur et une liste explicite de costumes. Les costumes vanilla appartiennent au personnage d'origine ; un personnage créé dans Visionary utilise les slots de sa `SlotClone`. Les costumes d'un même propriétaire partagent ses réglages.

Le résolveur (`src/roster/edit_scope.rs`) refuse les identités inconnues, les conflits de slots et les données vanilla insuffisantes. Une liaison sauvegardée doit correspondre exactement à la liaison actuelle. Les modifications anciennes non attribuées ou dont les slots ont changé restent sauvegardées mais inactives ; une attribution explicite est possible lorsque les portées sont compatibles.

## Édition et export

- Format projet v6 : banques ACMD séparées par propriétaire, avec sources, captures et neuf familles de données. Changer de propriétaire range la banque active et réinitialise l'historique d'annulation concerné.
- Les règles live de hitboxes, vitesse, sons, expressions, effets et cancels vérifient le combattant moteur et le costume. Les captures ACMD distinguent aussi les costumes. Les publications reconstruisent l'union des banques vérifiées, y compris après reconnexion.
- Les capacités TCP `scoped_acmd`, `scoped_cancels` et `scoped_aliases` sont négociées ; les enveloppes historiques servent uniquement à vider les règles.
- L'export ACMD utilise les quatre catégories (`game_`, `effect_`, `sound_`, `expression_`), préserve les identités des exports partiels et refuse les propriétaires ambigus ou les slots réattribués. Le dispatch Smashline est limité aux costumes du propriétaire. Les projets anciens restent lisibles ; les anciens éditeurs ne doivent pas ouvrir un projet enregistré en v6.
- Les Traits et EFF partagés ne sont pas isolables par simple garde de costume. Les Traits partagés sont bloqués ; les EFF historiques restent sauvegardables et exportables lorsque l'exclusivité est vérifiée. Les mécanismes EFF live globaux et pins sont désactivés. Les expériences EFF live sont conservées à part, non intégrées à cette version.

## Validation et limites

Les dix essais fonctionnels sur Ryujinx 1.1.1403 / Smash 13.0.5 ont été déclarés réussis sur le commit `964d402` avant retrait du prototype EFF désactivé. Ce retrait nécessite une vérification de compilation et un essai de non-régression. Deux personnages moddés partageant un combattant moteur et l'export autonome multi-propriétaires restent non validés en jeu. Les mods importés sans identité vérifiable ne bénéficient pas de cette garantie.

Les recherches EFF sont archivées sur `archive/eff-live-experimental` (commit `964d402`). Voir aussi la [documentation technique](https://github.com/Deklou/Smash_Modding_Docs/blob/main/techniques/ciblage_personnages.md).
