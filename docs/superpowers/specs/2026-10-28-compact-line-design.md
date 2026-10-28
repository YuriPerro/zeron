# Linha de compactação no feed

Data: 2026-10-28. Branch `feat/orchestrator-engine`.

## 1. Objetivo

Mostrar a compactação de contexto como uma linha horizontal na largura do chat, que fica no histórico. Durante a compactação a linha tem um loader e o texto "Compacting context". Ao terminar, mostra "Compacted" e quantos tokens o contexto tinha antes e depois.

Hoje o `/compact` aparece como bolha do usuário ("⌘ compact") seguida da palavra de trabalho ("Brewing…"). Ao terminar não sobra nada no feed. O compact automático no meio de uma tarefa nem aparece.

## 2. Decisões

| Tema | Decisão |
|---|---|
| Onde fica o registro | Item novo do histórico do chat (`MessagePart::Compaction`), gravado pelo motor. Sobrevive a reabrir o chat e sincroniza. |
| Providers | Claude Code e Codex. Os outros continuam como hoje. |
| Compact manual e automático | Os dois geram a mesma linha. |
| Bolha do `/compact` | Some quando o comando é só `/compact`, sem instruções, e a resposta tem a linha de compactação. Com instruções (`/compact keep the plan`) ou sem a linha, a bolha continua como hoje. |
| Palavra de trabalho | Não aparece enquanto a linha estiver em "Compacting context". |
| Texto da interface | Inglês, como o resto do Zeron. |
| Builds antigos | Ignoram o item novo, como já fazem com `fork` e `switch`. |

## 3. Estados da linha

```
Running              ───────────── ◌ Compacting context ─────────────
Done, antes e depois ──────── Compacted · 224K → 31K tokens ──────────
Done, só antes       ─────────── Compacted · from 224K tokens ─────────
Done, só depois      ────────── Compacted · down to 31K tokens ────────
Done, sem números    ──────────────────── Compacted ───────────────────
Running sem streaming ─────────────── Compaction stopped ──────────────
```

- Os números usam formato curto: abaixo de 1.000 o valor inteiro ("850"), em milhares arredondados ("224K"), a partir de 1 milhão com uma casa ("1.2M"). O indicador de contexto usa o valor completo ("224,463") e continua igual.
- "Compaction stopped": a resposta terminou (interrompida, com erro ou motor reiniciado) com a compactação ainda em andamento. A tela deduz isso de uma linha `running` numa resposta que não está mais em streaming. Nada extra é gravado.
- A linha não tem timestamp nem botão de copiar, como os marcadores de `fork` e `switch`.
- No compact automático no meio de uma resposta, a linha fica entre as partes da resposta, na ordem em que aconteceu. Ela nunca entra no grupo recolhido de ferramentas.

## 4. De onde vêm os números

| Provider | Início | Fim | Antes | Depois |
|---|---|---|---|---|
| Claude Code | frame `status` com `compacting` | frame `compact_boundary` | `compact_metadata.pre_tokens` | `post_tokens` (só no automático) |
| Codex | item `contextCompaction` iniciado | mesmo item concluído | última leitura de contexto do chat | primeira leitura depois do fim |

Conferido no registro do Claude Code 2.1.311: o compact automático traz `preTokens` e `postTokens`, o manual traz só `preTokens`.

Regra geral do motor:
- Quando o fim chega sem "antes", o motor usa a última leitura de contexto do chat.
- Quando o fim chega sem "depois", o motor guarda essa linha como pendente. A primeira leitura de contexto com tokens que chegar depois preenche o "depois". No `/compact` manual do Claude isso acontece quando o chat responde à próxima mensagem.
- A pendência não precisa de estado extra. No início de cada execução, o motor considera pendente a linha `Done` sem "depois" que estiver na última resposta do chat. Assim a pendência sobrevive a um reinício do motor.
- Quando o fim chega sem um início (o frame de status pode não vir), a linha nasce já concluída.

## 5. Dados

Evento novo do harness para o motor:

```
AgentEvent::Compaction { phase: Started | Finished, tokens_before: Option<u64>, tokens_after: Option<u64> }
```

Item novo do histórico:

```
MessagePart::Compaction { id, status: Running | Done, tokens_before: Option<u64>, tokens_after: Option<u64> }
```

- `Started` cria o item `Running` na resposta em andamento. Se já existe um `Running`, nada muda.
- `Finished` passa o último `Running` para `Done` e grava os números. Sem `Running`, cria um item já `Done`.
- No doc o item usa `kind: "compaction"`. O texto "Context compacted." que o Codex grava hoje deixa de existir.
- O frame `compacting` do Claude continua servindo de sinal de vida da resposta, como hoje.

## 6. Testes

- Normalização do Claude: `compacting` vira `Started`. `compact_boundary` automático vira `Finished` com antes e depois, e o manual vira `Finished` só com antes.
- Codex: o item `contextCompaction` iniciado e concluído vira `Started` e `Finished`, sem o texto "Context compacted.".
- Fold: `Started` e depois `Finished` dão um item `Done`. `Finished` sozinho dá um item `Done`. Um `Started` repetido não duplica.
- Ida e volta do item no doc (`kind: "compaction"`).
- Motor: um `Finished` sem "depois" é preenchido pela próxima leitura de contexto e só por ela.
- Tela: o texto de cada estado da seção 3. A bolha do `/compact` some só quando a resposta tem a linha. A palavra de trabalho some enquanto a linha está em `Running`.

## 7. Fora do escopo

- Pi, Muse e outros providers.
- Mostrar o resumo gerado pela compactação.
- Botão ou atalho novo para compactar.
