# Diagrams

A flowchart with a subgraph, several shapes and every edge style:

```mermaid
flowchart TD
    A[Markdown file] -->|save| B(ƜackDown)
    B --> C{Diagram?}
    C -->|yes| D[Drawn with Direct2D]
    C -->|no| E[Shown as code]
    D -.-> F([Printed as vectors])
    E ==> G[[Copy button]]
    subgraph Engine
        direction LR
        P[(Parser)] --> L{{Layout}} --> R((Scene))
    end
    B --- P
    G --o H>Done]
    H --x A
```

A sequence diagram:

```mermaid
sequenceDiagram
    autonumber
    participant Alice
    participant B as Bob
    Alice->>+B: Hello Bob, how are you?
    B-->>-Alice: Great!
    Alice->>Alice: Thinks
    Note over Alice,B: A note spanning both
    loop Every minute
        Alice-xB: Ping
        B-)Alice: Pong
    end
    alt Happy
        Alice->>B: Smile
    else Sad
        Alice->>B: Frown
    end
```

A class diagram:

```mermaid
classDiagram
    class Animal {
        +String name
        +int age
        +makeSound() void
        +isMammal()* bool
    }
    class Duck {
        +String beakColor
        +swim()
        +quack()
    }
    <<interface>> Swimmer
    Animal <|-- Duck
    Swimmer <|.. Duck
    Duck "1" *-- "many" Feather : has
    Duck o-- Pond
    Duck --> Food : eats
    Pond ..> Water
```

A state diagram:

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Loading : open file
    Loading --> Ready : parsed
    Loading --> Error : failed
    state Ready {
        [*] --> Reading
        Reading --> Searching : Ctrl+F
        Searching --> Reading : Esc
    }
    state choice <<choice>>
    Error --> choice
    choice --> Idle : retry
    choice --> [*] : quit
    Ready --> [*]
```

A pie chart:

```mermaid
pie showData title Time spent
    "Reading" : 55
    "Writing" : 25
    "Searching" : 10
    "Printing" : 7
    "Zooming" : 3
```

The newer flowchart syntax, an invisible link and a subgraph with its own direction:

```mermaid
flowchart TB
    S@{ shape: cyl, label: "Settings store" } --> T@{ shape: tri, label: "Pick" }
    T --> M@{ shape: manual-input, label: "Ask the user" }
    N@{ shape: text, label: "a plain note" } ~~~ M
    subgraph Pipeline
        direction LR
        p1[Parse] --> p2[Lay out] --> p3[Draw]
    end
```

An entity relationship diagram:

```mermaid
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE-ITEM : contains
    CUSTOMER }|..|{ DELIVERY-ADDRESS : uses
    CUSTOMER {
        string name PK
        string custNumber
        string sector "industry"
    }
    ORDER {
        int orderNumber PK
        string deliveryAddress FK
    }
```

A Gantt chart:

```mermaid
gantt
    title Release plan
    dateFormat YYYY-MM-DD
    section Engine
    Parser           :done,    p1, 2024-01-01, 7d
    Layout           :active,  l1, after p1, 10d
    Renderer         :          r1, after l1, 5d
    section Docs
    Write help       :crit,    d1, 2024-01-10, 4d
    Review           :milestone, m1, after d1, 0d
    Website          :          w1, 2024-01-15, 2024-01-22
```

A git graph:

```mermaid
gitGraph
    commit id: "init"
    commit id: "docs"
    branch develop
    checkout develop
    commit id: "feature A"
    commit id: "feature B" tag: "v0.1"
    checkout main
    commit id: "hotfix" type: HIGHLIGHT
    merge develop id: "release" tag: "v1.0"
    commit id: "next"
```

A mind map:

```mermaid
mindmap
  root((ƜackDown))
    Rendering
      Direct2D
      DirectWrite
    Markdown
      CommonMark
      GFM tables
      Math
    Diagrams
      Flowchart
      Sequence
      ::icon(fa fa-book)
      Many more
```

A timeline:

```mermaid
timeline
    title History of ƜackDown
    section Foundations
        2024-01 : Native Direct2D viewer
                : Themes and zoom
        2024-02 : Images and printing
    section Diagrams
        2024-03 : Flowcharts : Sequence : Class
        2024-04 : Everything else
```

A user journey:

```mermaid
journey
    title Reading a report
    section Open
      Double-click the file: 5: Reader
      Wait for the window: 5: Reader
    section Read
      Scroll and search: 4: Reader
      Hit a Mermaid block: 2: Reader, Author
      See it drawn: 5: Reader, Author
```

A quadrant chart:

```mermaid
quadrantChart
    title Reach and engagement
    x-axis Low reach --> High reach
    y-axis Low engagement --> High engagement
    quadrant-1 Expand
    quadrant-2 Promote
    quadrant-3 Re-evaluate
    quadrant-4 Improve
    Blog: [0.3, 0.6]
    Newsletter: [0.45, 0.23]
    Docs site: [0.57, 0.69]
    Release notes: [0.78, 0.34]
    Forum: [0.4, 0.34]
```

An XY chart:

```mermaid
xychart-beta
    title "Documents opened per month"
    x-axis [Jan, Feb, Mar, Apr, May]
    y-axis "Documents" 0 --> 120
    bar [35, 60, 72, 90, 110]
    line [20, 48, 65, 80, 105]
```

A broken one stays code:

```mermaid
graph TD
A -->
[[[
```

An unsupported type stays code:

```mermaid
sankey-beta
Agriculture,Bio-conversion,124.729
Bio-conversion,Liquid,0.597
```

Text after.
