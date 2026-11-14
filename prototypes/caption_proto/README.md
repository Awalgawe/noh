# caption_proto — prototype de mesure (non compilé)

Prototype wgpu puis OpenGL du moteur d'effets de sous-titres, qui a servi aux mesures des étapes 1 à 3 du chantier « effets ». Le moteur retenu est intégré dans `src/fx/` par le commit suivant.

- **Code jamais relu par le Critic.** Il est gardé ici comme trace des mesures, à la demande du mainteneur, et ne fait pas partie du produit.
- **Il n'est pas compilé.** Ce dossier n'est ni un exemple Cargo ni un membre d'espace de travail, et `Cargo.toml` ne déclare aucune de ses dépendances.
- **Pour le reconstruire**, le copier dans `examples/caption_proto/` et ajouter ses dépendances de développement : `wgpu = "=30.0.1"` (std, dx12, vulkan, gles, wgsl), `cosmic-text = "=0.19.0"`, `fdsm = "=0.8.0"`, `fdsm-ttf-parser = "=0.2.0"`, `nalgebra = "0.34"`, `ttf-parser = "0.25"`, `pollster = "0.4"`, `glutin = "=0.32.3"`, `glutin-winit = "=0.5.0"`, `winit = "0.30.13"` et `bytemuck = "1"`. Puis le compiler avec `--features gui`.
- **Archive identique**, avec empreintes : `D:\archive\nohh\caption_proto\`.
- **Dans les rapports approuvés**, il est cité sous `examples/caption_proto/` : c'est ce dossier.
