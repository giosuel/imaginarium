# 0013. Generative AI as a core, provider-agnostic subsystem

- Status: Accepted (implementation from M6, architecture from M0)
- Date: 2026-10-12

## Context
AI is a core feature: generating and rewriting text, generating and editing
images (reference images, img2img, sketch-to-image, inpainting and
outpainting, background removal, style transfer, upscaling, …), and an
assistant that reads and edits boards, documents, slides and vector
graphics. Users must be able to choose **local** models (Qwen, Mistral,
Flux 2, Stable Diffusion, …) or **API** models (Claude, ChatGPT, Gemini,
Mistral API, …).

## Decision

### 1. Capabilities, not vendors
`imaginarium-ai` defines capability traits. Providers implement the subset
they support, and the UI only shows features that the configured providers
can do.

| Capability | Examples |
|------------|----------|
| `TextGen` | chat, streaming, tool calls, structured output |
| `Vision` | image or region understanding (multimodal input) |
| `ImageGen` | txt2img, img2img (strength), inpaint/outpaint (mask), multi-reference, control inputs (sketch, depth, pose, edges), style reference, instruction editing |
| `ImageOps` | background removal, segmentation (click-to-mask), upscaling, object removal |
| `Embed` | semantic search across documents and templates |
| `Speech` (later) | dictation, narration |

### 2. Provider adapters (`imaginarium-ai-providers`)
- **API:** Anthropic (Claude), OpenAI, Google, Mistral, Black Forest Labs, and
  any **OpenAI-compatible** endpoint (OpenRouter and others).
- **Local LLM:** Ollama, llama.cpp server, LM Studio and vLLM, all through
  OpenAI-compatible HTTP. Embedding llama.cpp directly is a later option.
- **Local image:**
  - **ComfyUI**: workflows are shipped as versioned templates, which gives
    us Flux 2, SD/SDXL, ControlNet, IP-Adapter and inpainting.
  - **stable-diffusion.cpp**: an embedded option that needs no extra
    install.
- **Built-in small models** via ONNX Runtime (`ort`): background removal,
  segmentation and upscaling. They run fully offline and fast, with nothing
  else to install.
- Providers can also be **WASM add-ons** (with the network capability), so
  the community can add new ones without changing the core.

### 3. The assistant edits through commands
- Every command (PLAN §4.4) already has an id, metadata and an argument
  schema. The AI tool bridge exposes the available commands as **tools**,
  plus read tools: selection, node query, text extraction, and region render
  (rasterized for vision models).
- AI edits arrive as a **suggestion transaction**. The user previews a diff
  on the canvas or in the text and accepts or rejects it, as a whole or per
  change. Accepted edits are committed under the AI's peer id, so they are
  attributable and can be undone separately.
- **MCP:** Imaginarium can act as an **MCP server**, so Claude Desktop, Claude
  Code or other agents can drive an open document. It can also act as an
  **MCP client**, letting the assistant use the user's external tools.

### 4. Non-destructive generative image editing
- A `generated image` node stores its recipe (`gen` map, format §3.5),
  inputs, masks (raster layers) and all variants as blobs. Users can
  regenerate, make variations, branch or compare.
- Masks come from brush, selection, lasso, or click-to-segment.

### 5. Jobs, threading, privacy
- AI calls are **jobs**: async on a dedicated tokio runtime (the core stays
  runtime-free), cancellable, with progress and streamed partial results
  (tokens, preview images), and never on the UI thread.
- **AI policy** per document and workspace: `off`, `local only` or `any`.
  Shared documents respect the owner's policy. The UI always shows clearly
  when content is sent to a cloud provider.
- The hub is **never involved** in AI calls, so E2EE is preserved.
- API keys are stored in the OS keychain. Local models are managed in the UI:
  download, hardware/VRAM check, and delete.
- Prompt and workflow templates are packages (ADR-0006), so they can be
  shared and versioned.

## Consequences
- Supporting a new model usually means a new adapter or ComfyUI workflow,
  not core changes.
- The command schema becomes part of the AI contract, so commands need good
  descriptions and argument schemas. This also makes the palette better.
