# Lê-XML

Leitor e editor de arquivos **XML em formato de tabela**, nativo para Linux
(**GTK4 puro**). Abre o arquivo, mostra os dados numa grade estilo planilha
e permite **pesquisar**, **filtrar com SQL**, **somar colunas**, **editar
células**, **salvar de volta em `.xml`** (preservando a estrutura original) e
**exportar CSV**.

É leve e rápido: abre arquivos grandes (dezenas de milhares de linhas) quase
instantaneamente.

**Integração visual:** por usar GTK4 puro (sem libadwaita), o app **segue o
tema do sistema** — cores, cor de destaque (accent) e variante clara/escura
vêm do desktop (GNOME, Zorin OS etc.). Nas Configurações você ainda pode forçar
**Claro** ou **Escuro**, ou deixar em **Seguir o sistema**.

---

## O formato suportado

O app lê XMLs cuja estrutura separa **definição de colunas** de **dados**:

```xml
<DATAPACKET Version="2.0">
  <METADATA>
    <FIELDS>
      <FIELD attrname="id"    fieldtype="string" WIDTH="10"/>
      <FIELD attrname="nome"  fieldtype="string" WIDTH="40"/>
      <FIELD attrname="valor" fieldtype="string" WIDTH="20"/>
    </FIELDS>
    <PARAMS/>
  </METADATA>
  <ROWDATA>
    <ROW id="1" nome="Alpha" valor="10.50"/>
    <ROW id="2" nome="Beta"  valor="4.25"/>
    <ROW id="3" nome="Gama"/>
  </ROWDATA>
</DATAPACKET>
```

- Cada `<FIELD>` em `METADATA/FIELDS` vira uma **coluna**.
- Cada `<ROW>` em `ROWDATA` vira uma **linha** (cada atributo = uma célula).
- Atributos ausentes numa `<ROW>` são células vazias.

Ao salvar, o app **reescreve essa mesma estrutura**: os `<FIELD>` originais são
preservados exatamente como estavam; apenas os dados das linhas são regravados.

---

## Índice da documentação

- [Guia de uso](docs/uso.md) — como abrir, pesquisar, filtrar, somar, editar,
  salvar e exportar.
- [Build e empacotamento](docs/build-e-empacotamento.md) — compilar do código-fonte,
  gerar o AppImage e controlar o tema (Seguir o sistema / Claro / Escuro).

---

## Início rápido

```bash
# compilar
cargo build --release

# abrir um arquivo
./target/release/lexml-gtk caminho/arquivo.xml
```

Ou rode sem argumento e use o botão **Abrir arquivo**.

---

## Licença

Distribuído sob a **GNU General Public License v3.0 ou posterior**
(GPL-3.0-or-later). Veja o arquivo [LICENSE](LICENSE).
