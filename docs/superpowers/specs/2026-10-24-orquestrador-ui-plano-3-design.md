# Orquestrador: interface (Plano 3)

Data: 2026-10-24. Complementa `2026-10-24-orquestrador-design.md` (seção 10). O motor (Plano 1) já existe na branch `feat/orchestrator-engine`.

## 1. Objetivo

Abrir o Zeron e achar os orquestradores na hora. Na conversa, separar de relance o que é do usuário, o que é do agente e o que veio de outro chat. Criar orquestrador sem terminal.

Origem: teste de ponta a ponta do Plano 1. Dois incômodos:
- os avisos dos filhos aparecem como bolha do usuário (`[Zeron: ... finished its turn ...]`);
- o orquestrador fica misturado nas sessões comuns e os filhos só aparecem no popover "Side chats".

Fora deste plano (vai para o Plano 4): editar prompt pela tela, avatar pixel art, tela de papéis, papel no composer de chats comuns, lista de tarefas do orquestrador, resumo do que cada filho está fazendo.

## 2. Decisões

| Tema | Decisão |
|---|---|
| Como reconhecer mensagens entre chats | Pelo formato do texto, só na tela. Nada muda no que é gravado ou sincronizado. |
| Alcance | Toda mensagem entre chats, não só as do orquestrador. |
| Aviso de filho | Linha discreta e clicável que abre o filho. |
| Mensagem com conteúdo vinda de outro chat | Bolha à esquerda com avatar e nome de quem mandou. |
| Filhos na barra lateral | Só os ativos aninhados. Terminados em "N done", recolhido. |
| Filho esperando resposta | Passa pelo orquestrador. O selo "Needs you" fica só no orquestrador. |
| Texto da interface | Inglês, como o resto do Zeron. |
| Diferença com o Zeron original | Código novo em módulo próprio da interface. Arquivos originais recebem só ganchos curtos. |

## 3. Barra lateral

```
 All projects
 ORCHESTRATORS                 +
 ▾ [P] Plataforma          Needs you
     ⟳ arch-live: explorar
     ⟳ chess-arena: explorar
     ▸ 3 done
 ▸ [L] Loja                   ⟳
 SESSIONS
   ...
```

- Seção "Orchestrators" acima das sessões, com o cabeçalho que abre e fecha já usado em fixados, pastas e grupos por projeto. O estado aberto ou fechado é lembrado como o dos outros grupos.
- "+" no cabeçalho abre o formulário de criação (seção 5).
- Um orquestrador é todo chat não arquivado com `config.orchestrator` preenchido. Ele aparece só nesta seção, não em Sessions.
- Linha do orquestrador: avatar, título do chat (o nome) e um único indicador. Prioridade: "Needs you" (o chat está esperando resposta), depois trabalhando, depois erro, depois nada.
- Avatar: quadrado com a inicial e cor derivada do nome, usando o monograma que os ícones de projeto já usam. O campo `avatar` do orquestrador continua sem uso até o Plano 4.
- Filhos: chats não arquivados cujo pai é o chat do orquestrador.
  - Ativos (trabalhando, esperando resposta, erro) ficam aninhados, indentados, com o indicador normal de linha.
  - Os demais ficam numa linha "N done", recolhida por padrão. Abrir mostra a lista.
  - Clicar num filho abre o chat dele. Os filhos continuam também no popover "Side chats", sem mudança.
- Com a seção recolhida, o cabeçalho mostra quantos orquestradores precisam do usuário ("2 need you").
- Sem nenhum orquestrador, a seção mostra só o cabeçalho com o "+".

## 4. Mensagens na conversa

Uma mensagem de usuário é classificada pelo texto completo, antes de desenhar:

| Formato (gerado pelo próprio Zeron) | Vira |
|---|---|
| `[Zeron: <title> (<id>) finished its turn. Read it with read_chat.]` | Linha de aviso, cinza: `<title> finished` |
| `[Zeron: <title> (<id>) is waiting for an answer. Read its question with get_chat.]` | Linha de aviso, âmbar: `<title> is waiting for an answer` |
| `[Zeron: <title> (<id>) stopped with an error. Read it with read_chat.]` | Linha de aviso, vermelha: `<title> stopped with an error` |
| `[Message from Zeron chat <label>. Reply to it with the Zeron \`send_message\` tool, chat <id>.]\n\n<body>` | Bolha à esquerda, cabeçalho com avatar e nome do chat `<id>`, corpo `<body>` |
| `[Contract change requested by <title> (<id>) for demand <demand>]\n\n<reason>` | Bolha à esquerda do chat `<id>`, etiqueta "Contract change · <demand>", corpo `<reason>` |
| Qualquer outro texto | Bolha do usuário, como hoje |

- A linha de aviso usa o mesmo tipo de linha dos marcadores de fork e troca de provedor. Mostra um ponto colorido, o texto e "Open". Clicar abre o chat `<id>`.
- Um aviso só é reconhecido quando o texto inteiro casa com o formato. O `<id>` é o último par de parênteses antes do verbo, então títulos com parênteses funcionam.
- A bolha com remetente usa o título atual do chat `<id>` e o avatar dele: o monograma do orquestrador quando o remetente é orquestrador, o ícone do provedor nos demais casos. Se o chat `<id>` não existir mais, usa o `<label>` do texto.
- O Zeron original já reconhece o formato "Message from Zeron chat" e põe o nome em negrito dentro da bolha do usuário. Esse tratamento é substituído pela bolha à esquerda.
- Se algum formato mudar no futuro, a mensagem volta a aparecer como bolha do usuário. Nada quebra.

## 5. Criar orquestrador

```
 ┌ New orchestrator ─────────────────────────────── × ┐
 │ Name       [ Plataforma                       ]    │
 │ Projects   [ Search projects… ] [Add folder…]      │
 │            ┌──────────────────────────────────┐    │
 │            │ [x] kit-ui       ~/dev/kit-ui    │    │
 │            │ [x] chess-arena  ~/dev/pers…     │    │
 │            │ [ ] arch-live    Folder not found│    │
 │            └──────────────────────────────────┘    │
 │            2 selected                              │
 │ Provider   ( Claude Code ▾ )                       │
 │ Model      ( Opus 5.5 ▾ ) ( High ▾ )               │
 │ Children   ( Same provider ▾ )                     │
 │                             [Cancel] [Create]      │
 └────────────────────────────────────────────────────┘
```

- Janela no padrão de diálogo que o Zeron já usa (criar pasta, renomear projeto).
- Projects: lista com caixas de marcar dos projetos deste computador. O motor só aceita projetos locais.
- Busca: o campo "Search projects…" filtra a lista pelo nome ou pelo caminho, sem diferenciar maiúsculas. Projetos marcados continuam marcados quando saem do filtro. Sem resultado, a lista mostra "No projects match.".
- Caminho: cada linha mostra o nome do projeto e a pasta em cinza, com a pasta pessoal escrita como "~" e o excesso cortado com reticências.
- Pasta ausente: a existência da pasta é verificada ao abrir a janela e quando a lista de projetos muda. Se a pasta não existe mais, a linha fica desligada, não pode ser marcada, mostra "Folder not found" no lugar do caminho e vai para o fim da lista.
- Add folder…: abre o seletor de pastas do sistema e registra a pasta escolhida como projeto neste computador, pelo mesmo pedido que "Adicionar projeto" usa. Se a pasta já é um projeto, ela só é marcada. Um projeto novo é marcado assim que aparece na lista. Se o registro falha, a mensagem do erro aparece dentro da janela.
- Contagem: abaixo da lista, "N selected" em cinza, escondido quando N é zero.
- Provider: só Claude Code e Codex, e só os conectados. O motor recusa os demais.
- Model: modelos do provedor escolhido, com o nível de raciocínio ao lado. Padrão: o modelo padrão do provedor.
- Children: "Same provider" ou outro provedor conectado.
- Create chama `CreateOrchestrator`. No sucesso, fecha a janela e abre o chat novo. No erro, mostra a mensagem do motor dentro da janela e mantém o que foi preenchido.
- Validação antes de enviar: "Name is required." e "Select at least one project.". O botão Create fica desligado enquanto envia.

## 6. Selo "Needs you"

- Acende quando o chat do orquestrador está esperando resposta do usuário (painel de pergunta ou permissão).
- Filho esperando resposta não acende selo próprio: o motor acorda o orquestrador, que responde ou pergunta ao usuário.
- Notificações do sistema seguem o comportamento atual de chat esperando resposta.

## 7. Código

- Módulo novo na interface para tudo que é do orquestrador: agrupamento da barra, linhas, classificação de mensagens, formulário.
- Ganchos curtos nos arquivos originais: um ponto na montagem da barra lateral, um na montagem das linhas da conversa, um no "+" e um filtro para tirar o orquestrador de Sessions.
- Lógica que dá para testar sem tela fica em funções puras: classificação de texto, agrupamento da barra (dado a lista de chats e o estado de cada um, devolve orquestradores, filhos ativos e contagem de terminados) e validação do formulário.

## 8. Testes

- Unitários: cada formato da tabela da seção 4, incluindo título com parênteses, texto parecido que não casa e corpo vazio. Agrupamento com orquestrador sem filhos, filhos arquivados, filho com erro e chat comum com pai que não é orquestrador. Validação do formulário.
- Teste de interface onde o Zeron já tiver padrão para isso (linhas da conversa ou barra lateral).
- Manual no Zeron Dev isolado: criar orquestrador pela tela, repetir a investigação do teste do Plano 1, conferir linhas de aviso, bolha com remetente no filho, seção na barra e selo.

## 9. Riscos

- O Zeron original mexe muito na barra lateral e na conversa. Ganchos pequenos reduzem conflito, mas cada atualização pode exigir ajuste.
- Se o Zeron original mudar o texto "Message from Zeron chat", a bolha com remetente para de aparecer até ajustar o reconhecimento.
- O app de iPhone continua mostrando tudo como hoje.
