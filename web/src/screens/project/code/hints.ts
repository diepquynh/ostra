// Completions, signature help, and supertype or implementation jumps for the file editor, asked over the socket
// with the unsaved text. The providers are registered once for every language; a model answers only while
// `bindModel` names its project file.

import { socket } from "../../../api/socket";
import type { CodeDoc, CodeRange, CompletionKind, NavigateTarget } from "../../../api/types";
import { monaco } from "../../../components/monaco";
import { askJump, type JumpReply } from "./jump";

type Model = monaco.editor.ITextModel;
type Where = { workspace: string; key: string; path: string };

const bound = new WeakMap<Model, Where>();

/** Answer hints in `model` as project file `path`. */
export function bindModel(model: Model, where: Where) {
  bound.set(model, where);
}

const K = monaco.languages.CompletionItemKind;
const KIND: Record<CompletionKind, monaco.languages.CompletionItemKind> = {
  text: K.Text,
  method: K.Method,
  function: K.Function,
  constructor: K.Constructor,
  field: K.Field,
  variable: K.Variable,
  class: K.Class,
  interface: K.Interface,
  module: K.Module,
  property: K.Property,
  unit: K.Unit,
  value: K.Value,
  enum: K.Enum,
  keyword: K.Keyword,
  snippet: K.Snippet,
  color: K.Color,
  file: K.File,
  reference: K.Reference,
  folder: K.Folder,
  enum_member: K.EnumMember,
  constant: K.Constant,
  struct: K.Struct,
  event: K.Event,
  operator: K.Operator,
  type_parameter: K.TypeParameter,
};

// Characters that open completions or signature help in some language. The server answers an empty list after a
// character its language does not use.
const COMPLETION_TRIGGERS = [".", ":", ">", "/", "@", "#", "<", '"', "'"];
const SIGNATURE_TRIGGERS = ["(", ",", "<"];

// Ostra columns are 0-based; Monaco columns are 1-based.
const toRange = (r: CodeRange) => new monaco.Range(r.line, r.col + 1, r.end_line, r.end_col + 1);
const toDoc = (d: CodeDoc | null | undefined) => (d ? (d.markdown ? { value: d.text } : d.text) : undefined);

function ask(model: Model, position: monaco.Position) {
  const where = bound.get(model);
  if (!where) return null;
  return { ...where, text: model.getValue(), line: position.lineNumber, col: position.column - 1 };
}

/** Supertypes or implementations of the name at `position`; null when the model is unbound. See `askJump`. */
export async function navigate(
  model: Model,
  position: monaco.Position,
  target: NavigateTarget,
): Promise<JumpReply | null> {
  const where = bound.get(model);
  if (!where) return null;
  return askJump(where, model.getValue(), position.lineNumber, position.column - 1, target);
}

let registered = false;

/** Register the providers once. Call it before an editor mounts. */
export function registerHints() {
  if (registered) return;
  registered = true;
  monaco.languages.registerCompletionItemProvider("*", {
    triggerCharacters: COMPLETION_TRIGGERS,
    async provideCompletionItems(model, position, context, token) {
      const base = ask(model, position);
      if (!base) return undefined;
      const Trigger = monaco.languages.CompletionTriggerKind;
      const reply = await socket().hint("code_complete", {
        ...base,
        trigger: context.triggerKind === Trigger.TriggerCharacter ? (context.triggerCharacter ?? null) : null,
        retrigger: context.triggerKind === Trigger.TriggerForIncompleteCompletions,
      });
      const c = reply?.result;
      if (!c || token.isCancellationRequested) return undefined;
      const word = model.getWordUntilPosition(position);
      const here = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, position.column);
      return {
        incomplete: c.incomplete,
        suggestions: c.items.map((i) => ({
          label: i.label,
          kind: i.kind ? KIND[i.kind] : K.Text,
          detail: i.detail ?? undefined,
          documentation: toDoc(i.doc),
          insertText: i.insert,
          insertTextRules: i.snippet ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
          range: i.range ? toRange(i.range) : here,
          filterText: i.filter ?? undefined,
          sortText: i.sort ?? undefined,
          preselect: i.preselect,
          tags: i.deprecated ? [monaco.languages.CompletionItemTag.Deprecated] : undefined,
          additionalTextEdits: i.edits?.map((e) => ({ range: toRange(e.range), text: e.text })),
          commitCharacters: i.commit_chars,
        })),
      };
    },
  });
  monaco.languages.registerSignatureHelpProvider("*", {
    signatureHelpTriggerCharacters: SIGNATURE_TRIGGERS,
    signatureHelpRetriggerCharacters: [")"],
    async provideSignatureHelp(model, position, token, context) {
      const base = ask(model, position);
      if (!base) return undefined;
      const reply = await socket().hint("code_signature", {
        ...base,
        trigger: context.triggerCharacter ?? null,
        retrigger: context.isRetrigger,
      });
      const h = reply?.result;
      if (!h || token.isCancellationRequested) return undefined;
      return {
        value: {
          activeSignature: h.active_signature,
          activeParameter: h.active_parameter,
          signatures: h.signatures.map((s) => ({
            label: s.label,
            documentation: toDoc(s.doc),
            activeParameter: s.active_parameter ?? undefined,
            parameters: s.parameters.map((p) => ({
              label: [p.start, p.end] as [number, number],
              documentation: toDoc(p.doc),
            })),
          })),
        },
        dispose() {},
      };
    },
  });
}
