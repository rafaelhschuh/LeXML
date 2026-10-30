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
de `FIELDS` com um atributo `attrname` não vazio e que seja um **nome válido de
atributo XML** (veja [Nomes de coluna](#nomes-de-coluna)). Um `attrname` fora
dessa regra faz o arquivo abrir no modo texto, com o motivo no rodapé.

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
mantida em uma coluna interna de ordenação, que não aparece na grade nem no
arquivo salvo. Uma segunda coluna interna identifica a linha durante a edição e
também não é mostrada.

Essas colunas usam por padrão os nomes `__ord`, `__rid` e `_rid`. Se o arquivo
trouxer um `FIELD` com algum desses nomes — ou com `rowid`, que o SQLite
reserva —, o aplicativo renomeia a coluna **interna**, não a do arquivo. Um
documento com uma coluna chamada `__ord` abre normalmente e sobrevive ao
round-trip.

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

Os caracteres `&`, `<`, `>`, `"` e `'` dos **valores** são escapados. As
quebras de linha, retorno de carro e tabulação são gravadas como referências
numéricas (`&#10;`, `&#13;`, `&#9;`) para sobreviver à próxima leitura.

Os **nomes** de atributo não podem ser escapados — não existe escape para eles
na sintaxe XML. Por isso a validação acontece na entrada, e não na saída: veja
a seção seguinte.

## Nomes de coluna

O nome de uma coluna é gravado como **nome de atributo** dos elementos `ROW`,
então precisa ser um `Name` válido do XML 1.0:

| Posição | Caracteres aceitos |
| --- | --- |
| Primeiro caractere | letra (inclusive acentuada), `_` ou `:` |
| Demais caracteres | letra, dígito, `_`, `-`, `.` ou `:` |

São aceitos: `valor_total`, `data.emissão`, `cnpj-cpf`, `Número`.

São recusados: nome vazio, `valor total` (espaço), `2024` (começa com dígito),
`a"b` (aspas), `<tag>`.

A regra é aplicada em três momentos: ao ler o `METADATA`, ao renomear ou criar
uma coluna pela interface, e como última verificação na gravação. Um nome fora
da regra produziria um arquivo malformado — no caso de aspas, permitiria até
injetar atributos ou elementos no XML salvo, que seguiria assim para o sistema
de destino.

> Essa validação é independente da proteção da camada SQL. Nomes com acentos,
> palavras reservadas ou caracteres especiais continuam sendo escapados ao
> montar as instruções internas, e no campo de filtro o autocompletar adiciona
> aspas duplas quando necessário:
>
> ```sql
> "valor total" <> ''
> ```

Não podem existir duas colunas com o mesmo nome — a comparação ignora
maiúsculas e minúsculas, como o SQLite faz. O documento também precisa manter
ao menos uma coluna.

## Modo texto

Se a leitura tabular falhar — por exemplo, por não existir `FIELDS` válido ou
por um `attrname` inválido — o arquivo é aberto no editor de texto simples, com
o motivo indicado no rodapé.

Salvar grava o conteúdo exibido no editor e não aplica as regras de
reconstrução do modo tabela.

O editor espera UTF-8. Quando o arquivo **não é UTF-8 válido**, ou passa de
**64 MiB**, ele abre somente para leitura: o texto mostrado não corresponde
byte a byte ao original (os bytes inválidos aparecem como `?`, ou o conteúdo
foi cortado no teto), e gravá-lo destruiria o arquivo em silêncio.

## Recomendações

- Faça o primeiro salvamento com outro nome ao validar um formato novo.
- Use UTF-8. Converta antes os arquivos em Latin-1:
  `iconv -f ISO-8859-1 -t UTF-8 entrada.xml > saida.xml`.
- Declare cada coluna com um `attrname` que seja um nome válido de atributo XML.
- Mantenha cada coluna declarada em `FIELDS`.
- Não dependa da preservação de comentários ou formatação visual.
- Se a preservação literal do documento for necessária, trabalhe no modo texto.

Voltar ao [índice da documentação](README.md).
