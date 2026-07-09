# Arche OIDC — Documentation

Both halves of OpenID Connect in `arche::oidc`:

- **`arche::oidc` (client)** — log your users in *via someone else* ("Sign in
  with Google"). You are the **relying party**.
- **`arche::oidc::server`** — log other apps' users in *via you* ("Sign in
  with your service"). You are the **identity provider**.

They are mirror images and never share code; the interop test proves they
speak the same dialect by having the client complete a full login against the
server.

| Doc | Read when |
|---|---|
| [architecture.md](architecture.md) | You want the mental model: the two halves, which types live where, the four server trait-seams, and which of them ship a built-in. Includes a component diagram with hover tooltips. |
| [sequence.md](sequence.md) | You want to know what actually happens during a login — the authorization-code + PKCE flow, both directions, and what `state`/PKCE defend against. Includes sequence diagrams. |
| [extending.md](extending.md) | You want to run a client, stand up the server, or implement one of the four server traits (client registry, token signer, access-token issuer, code store). |

Hand-editable visual — one canvas, two panels (the runtime login journey on
the left, the static type/trait model on the right). Open at
[excalidraw.com](https://excalidraw.com) → File → Open, or the VS Code
Excalidraw extension:

- [`oidc.excalidraw`](oidc.excalidraw)

Quick jumps:

- **"I just want Sign in with Google"** → [extending.md → Client quickstart](extending.md#client-quickstart)
- **"How do I stand up the IdP?"** → [extending.md → Server quickstart](extending.md#server-quickstart)
- **"Which trait do I implement, and how?"** → [extending.md → The four seams](extending.md#the-four-seams)
- **"Which type goes where?"** → [architecture.md → Map](architecture.md#map)
- **"What is `state` / PKCE actually defending?"** → [sequence.md → Why state and PKCE](sequence.md#why-state-and-pkce)
