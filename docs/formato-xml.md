# Formato XML suportado

O Lê-XML possui dois modos de abertura:

- **tabela**, para documentos no formato `DATAPACKET`;
- **texto**, usado como alternativa quando a estrutura tabular não é
  reconhecida.

Este documento descreve o modo tabela e o que acontece no ciclo
XML → SQLite em memória → XML.

## Estrutura mínima

```xml
<DATAPACKET Version="2.0">
  <METADATA>
    <FIELDS>
      <FIELD attrname="id" fieldtype="string" WIDTH="10"/>
      <FIELD attrname="descricao" fieldtype="string" WIDTH="80"/>
    </FIELDS>
    <PARAMS/>
  </METADATA>
  <ROWDATA>
    <ROW id="1" descricao="Primeiro registro"/>
    <ROW id="2"/>
  </ROWDATA>
</DATAPACKET>
```

Para abrir como tabela, o documento precisa conter ao menos um `FIELD` dentro
de `FIELDS` com um atributo `attrname` não vazio.

Os nomes das tags e do atributo `attrname` são sensíveis a maiúsculas e
minúsculas:

- `FIELDS`, `FIELD` e `ROW` devem aparecer em maiúsculas;
- deve ser usado `attrname`, em minúsculas.

## Conversão para a tabela

| XML | Tabela |
| --- | --- |
| `FIELD@attrname` | nome da coluna |
| demais atributos de `FIELD` | metadados preservados para o salvamento |
| elemento `ROW` | linha |
| atributo de `ROW` | valor da célula com o mesmo nome |
| atributo ausente | célula vazia |

Todos os valores são importados para colunas SQLite do tipo `TEXT`. O tipo
declarado em `fieldtype` é preservado no XML, mas não muda o tipo interno da
coluna.

A ordem dos `FIELD` define a ordem inicial das colunas. A ordem dos `ROW` é
mantida em uma coluna interna chamada `__ord`, que não aparece na grade nem no
arquivo salvo.

Uma segunda coluna interna, `_rid`, identifica a linha durante a edição e
também não é mostrada.

Atributos de `ROW` sem um `FIELD` correspondente são ignorados no modo tabela.

## O que é preservado ao salvar

O salvamento tabular preserva semanticamente:

- a tag de abertura de `DATAPACKET`, incluindo `Version` e outros atributos;
- cada `FIELD` reconhecido e seus atributos;
- a seção `PARAMS`, inclusive quando contém elementos internos;
- os nomes, valores e ordem atual das colunas;
- os valores e a ordem atual das linhas;
- caracteres especiais e quebras de linha presentes nos valores.

Ao renomear uma coluna, seu atributo `attrname` também é alterado. Colunas
novas recebem os atributos:

```xml
<FIELD attrname="colunaN" fieldtype="string" WIDTH="40"/>
```

## Normalização na saída

O Lê-XML não modifica o arquivo XML original enquanto você trabalha. Ao salvar,
ele gera um novo texto XML a partir da tabela em memória.

Por isso, não são preservados exatamente:

- indentação;
- espaços entre tags;
- quebras de linha entre elementos;
- declaração XML original;
- comentários, instruções de processamento e elementos não modelados;
- atributos de `ROW` sem coluna correspondente;
- distinção entre atributo ausente e atributo com valor vazio.

A saída usa esta declaração:

```xml
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
```

Valores vazios não geram atributos. Por exemplo, uma célula `descricao` vazia é
salva como `<ROW id="2"/>`, e não como `<ROW id="2" descricao=""/>`.

Os caracteres `&`, `<`, `>`, aspas e controles de atributo são escapados. As
quebras de linha, retorno de carro e tabulação são gravadas como referências
numéricas para sobreviver à próxima leitura.

## Nomes de coluna

Nomes com espaços, acentos, palavras reservadas ou aspas são protegidos ao
montar as instruções SQL internas. No campo de filtro, o autocompletar adiciona
aspas duplas quando necessário:

```sql
"valor total" <> ''
```

Não podem existir duas colunas com o mesmo nome. O documento também precisa
manter ao menos uma coluna.

## Modo texto

Se a leitura tabular falhar — por exemplo, por não existir `FIELDS` válido — o
arquivo é aberto no editor de texto simples.

Nesse modo, o conteúdo é decodificado como UTF-8 com substituição tolerante de
bytes inválidos. Salvar grava o conteúdo exibido no editor e não aplica as
regras de reconstrução do modo tabela.

## Recomendações

- Faça o primeiro salvamento com outro nome ao validar um formato novo.
- Use UTF-8.
- Mantenha cada coluna declarada em `FIELDS`.
- Não dependa da preservação de comentários ou formatação visual.
- Se a preservação literal do documento for necessária, trabalhe no modo texto.

Voltar ao [índice da documentação](README.md).
