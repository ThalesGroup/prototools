<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

I consider submitting the prototools at grehach 2026.
https://cfp.grehack.fr/grehack-2026/cfp
The deadline for submission is August 31st.

1. I have the following ideas for food:

- Use the gRPConf 2026 prototools video as support document (together with a written synopsis)
- Recall the context of "Cloud de confiance"

- Angles d'attaque:
  - Analyser les anomalies présentes dans un protobuf -> e.g., taxonomie des anomalies possibles
  - Rétro-concevoir un schema protobuf (reverse engineering) -> auto-inference et concept d'override
  - Performances -> avec un outil interactif, l'utilisateur progresse par essais/erreurs; il est critiques que les essais soient peu couteux. Il y a aussi la présentation d'indices visuels qui aident à la prise de décision et demandent l'exécution d'algorithmes en arrière plan

- À propos des techniques d'optimisation utilisées pour les prototools (non exhaustif)
  - Identifier les intangibles dans la dychotomie wire-level / proto-level:
    - Les noeuds sont toujours aux mêmes offsets (figés par la structure wire)
    - Les shadowing possibles sont figés par la structure wire
    - Le parcours de l'arbre proto est imposé par la structure wire (au nesting près)
    --> Les intangibles peuvent être calculés une fois et mis en cache, quel que soit l'override de niveau proto
  - La production de graphes inclut une étape de simplification (déduplication Hopcroft)
  - Des indexes sont pré-calculés pour permettre un accès JIT aux descripteurs dont les prototools ont besoin
  - D'une manière générale les techniques de caching et les approches paresseuses (JIT) sont très utilisées: par exemple le rendering du document a lieu à la demande, par rapport au viewport courant. Le document n'est jamais matérialisé dans son entièreté. Idem pour la fonction search. Elle utilise une abstraction de curseur et une représentation du document sous un forme "rope". Le but est le même: ne jamais avoir besoin de matérialiser le document.
  - Utilisation du multi-thread mappé sur du multi-cpu. L'inférence de type est shardée (e.g. sur 12 cpus).
  - Les caches sont eux chargé de manière pro-active (le contraire du JIT), mais ce chargement a lieu en tâche de fond, exécuté sur des threads disponibles
  - Utilisation du langage Rust

2. Dans ce cadre les prototools sont:

- reproto (CLI)
  - Décompile des FDP binaries pour restituer le proto initial
  - Rendu essentiellement 100% fidèle (à ré-arrangement près, et disparition des commentaires)
  - Les .proto décompilés peuvent être recompilés (round-trip fidèle)
  - reproto peut travailler à partir de données incomplètes
  - reproto peut aussi à la demande élaguer le source restitué par rapport aux FDP disponibles
  - support des syntaxes proto2 (y compris structures deprecated), proto3, editions
  - reproto peut aussi transcrire un FDP d'une syntaxe vers une autre, avec des restrictions si la syntaxe cible n'est pas aussi expressive que la syntaxe d'origine. Cela est par exemple utile si on veut ré-utiliser un source "editions" avec prost (qui ne supporte pour l'instant que proto2 et proto3)
  - reproto produit plusieurs types d'artifacts:
    - arborescence de sources .proto (possiblement élaguées par rapport au matériel FDP d'origine, en fonction des besoins de l'utilisateur)
    - descripteur set consolidé correspondant au même contenu
    - index vers le descripteur pour accès rapide aux types
    - graphe d'inférence pour auto-inférer le type d'un protobuf inconnu

- protoscan (CLI)
  - utilise le graphe d'inférence du type FileDescriptorProto et quelques heuristiques simples, pour identifier les FDP présents dans un fichier binaire.
    - démarrage par 0xA0
    - aucun codage non-canonique
  - protoscan est performant: typiquement scanne 1GiB/s

- prototext (CLI)
  - Effectue la conversion binaire <-> texte des protobufs (ser / deser)
  - La sérialisation s'appuie sur un schéma, mais ne suppose pas que le schéma corresponde au protobuf. prototext va désérialiser le protobuf binaire et va noter toutes les anomalies rencontrées, sous forme d'annotations -> le résultat est un fichier au format textproto, lisible, avec des annotation sous forme de commentaires #@.
  - La sérialisation est fidèle / lossless au sens ou le round-trip ser/deser redonne le binaire de départ au bit près, y compris dans le cas où on serait parti d'un binaire qui ne serait même pas un protobuf.
  - Dans le cas où l'utilisateur ne fournit pas de schéma, prototext peut utiliser un graphe d'inférence et son descriptor set associé, pour auto-inférer le type d'un protobuf inconnu, et sérialiser à partir de ce type auto-inféré.

-  protolens (TUI)
   - la version interactive de prototext - sous stéroides
   - affiche le protobuf sous forme d'arbre
   - coloriage des noeuds en fonction de la sévérité des anomalies (correct, champ masqué, champ inconnu, non-canonique, invalide)
   - affichage de heat-cues avec gradation de confiance: protolens indique les types qu'il reconnait à partir de sa fonction d'auto-inférence
   - concept d'override: l'utilisateur peut modifier le rendu du protobuf en appliquant un type donné à un sous-noeud
   - affichage du binaire en // du prototext
   - fonctions de sauvegarde des overrides et d'export de rendus
   - les rendus exportés peuvent être reconvertis fidèlement en le protobuf d'origine
   - protolens est performant: ouverture du googleapi.desc utilisé lui-même comme son propre descriptor set, en 0.7 secondes. (25 MiB, 8000 FDPs)

3. Look at the web page for GreHack 2026 call for paper
https://cfp.grehack.fr/grehack-2026/cfp

Identify opportunities for submitting.
What scope / subject?
What should be produced exactly for submitting?