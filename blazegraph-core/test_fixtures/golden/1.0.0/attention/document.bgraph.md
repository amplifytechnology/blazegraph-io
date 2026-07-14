```bgraph
{"schema":"1.0.0","kind":"document","blazegraph_version":"0.4.0","source":{"format":"pdf","filename":"attention.pdf","sha256":"bdfaa68d8984f0dc02beaca527b76f207d99b666d31d1da728ee0728182df697"},"flow_type":"Fixed","config_hash":"6daf2782c704bd37307850ae43bdcbbe5982a6a0ac08b1c5104de3e78738d5ce","graph_sha256":"884942cfe8a12c90921b4c857c6a82a7724faa4674856b8f96f22ba1233d09b1"}
```

```bgraph-metadata
{"title":"Attention Is All You Need","author":null,"description":null,"language":null,"created":"2024-04-10T21:11:43Z","pdf":{"version":"1.5","producer":"pdfTeX-1.40.25","creator_tool":"LaTeX with hyperref","publisher":null,"page_count":15,"encrypted":false,"has_marked_content":false,"modified":"2024-04-10T21:11:43Z","extras":{"Content-Type":"application/pdf","X-TIKA:versionCount":"0","access_permission:assemble_document":"true","access_permission:can_modify":"true","access_permission:can_print":"true","access_permission:can_print_faithful":"true","access_permission:extract_content":"true","access_permission:extract_for_accessibility":"true","access_permission:fill_in_form":"true","access_permission:modify_annotations":"true","dc:format":"application/pdf; version=1.5","pdf:docinfo:created":"2024-04-10T21:11:43Z","pdf:docinfo:creator_tool":"LaTeX with hyperref","pdf:docinfo:custom:PTEX.Fullbanner":"This is pdfTeX, Version 3.141592653-2.6-1.40.25 (TeX Live 2023) kpathsea version 6.3.5","pdf:docinfo:modified":"2024-04-10T21:11:43Z","pdf:docinfo:producer":"pdfTeX-1.40.25","pdf:docinfo:trapped":"False","pdf:eofOffsets":"2215244","pdf:hasCollection":"false","pdf:hasXFA":"false","pdf:hasXMP":"false","pdf:incrementalUpdateCount":"0"}}}
```

```bgraph-outline
{"sections":[{"title":"Introduction","order":0,"level":1},{"title":"Background","order":1,"level":1},{"title":"Model Architecture","order":2,"level":1},{"title":"Encoder and Decoder Stacks","order":3,"level":2},{"title":"Attention","order":4,"level":2},{"title":"Scaled Dot-Product Attention","order":5,"level":3},{"title":"Multi-Head Attention","order":6,"level":3},{"title":"Applications of Attention in our Model","order":7,"level":3},{"title":"Position-wise Feed-Forward Networks","order":8,"level":2},{"title":"Embeddings and Softmax","order":9,"level":2},{"title":"Positional Encoding","order":10,"level":2},{"title":"Why Self-Attention","order":11,"level":1},{"title":"Training","order":12,"level":1},{"title":"Training Data and Batching","order":13,"level":2},{"title":"Hardware and Schedule","order":14,"level":2},{"title":"Optimizer","order":15,"level":2},{"title":"Regularization","order":16,"level":2},{"title":"Results","order":17,"level":1},{"title":"Machine Translation","order":18,"level":2},{"title":"Model Variations","order":19,"level":2},{"title":"English Constituency Parsing","order":20,"level":2},{"title":"Conclusion","order":21,"level":1}]}
```

Provided proper attribution is provided, Google hereby grants permission to reproduce the tables and figures in this paper solely for use in journalistic or scholarly works.
```bgraph-paragraph
{"id":"c7c85e3c-cad3-5e8f-b177-d2dc177974be","node_type":"Paragraph","location":{"semantic":{"path":"1","depth":1,"breadcrumbs":["Attention Is All You Need"]},"physical":{"page":1,"bounding_box":{"x":124.3,"y":73.8,"width":363.60004,"height":38.499992}}},"text_order":0,"token_count":42,"style":null}
```

# Attention Is All You Need
```bgraph-section
{"id":"045d83c7-7081-569a-bf7e-4027a9d8e86f","node_type":"Section","location":{"semantic":{"path":"2","depth":1,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need"]},"physical":{"page":1,"bounding_box":{"x":211.5,"y":149.1,"width":188.4,"height":16.5}}},"text_order":1,"token_count":6,"style":null}
```

Ashish Vaswani ∗ Noam Shazeer ∗ Niki Parmar ∗ Jakob Uszkoreit ∗ Google Brain Google Brain Google Research Google Research avaswani@google.com noam@google.com nikip@google.com usz@google.com
```bgraph-paragraph
{"id":"66d3bd22-a939-5f16-b9cc-7988c93c71a2","node_type":"Paragraph","location":{"semantic":{"path":"2.1","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need"]},"physical":{"page":1,"bounding_box":{"x":116.7,"y":234.1,"width":380.5,"height":32.399994}}},"text_order":2,"token_count":43,"style":null}
```

Llion Jones ∗ Aidan N. Gomez ∗ † Łukasz Kaiser ∗ Google Research University of Toronto Google Brain llion@google.com aidan@cs.toronto.edu lukaszkaiser@google.com
```bgraph-paragraph
{"id":"6d1fe0e5-9874-51c8-b469-fa11e82b4428","node_type":"Paragraph","location":{"semantic":{"path":"2.2","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need"]},"physical":{"page":1,"bounding_box":{"x":126.9,"y":284.1,"width":358.19998,"height":32.399994}}},"text_order":3,"token_count":36,"style":null}
```

Illia Polosukhin ∗ ‡ illia.polosukhin@gmail.com
```bgraph-paragraph
{"id":"028997bf-9f47-5ed9-bb02-2ef6b6d4901a","node_type":"Paragraph","location":{"semantic":{"path":"2.3","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need"]},"physical":{"page":1,"bounding_box":{"x":238.0,"y":334.1,"width":136.0,"height":21.5}}},"text_order":4,"token_count":11,"style":null}
```

## Abstract
```bgraph-section
{"id":"7fca7229-15cd-5372-9dab-21090cf52eac","node_type":"Section","location":{"semantic":{"path":"2.4","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","Abstract"]},"physical":{"page":1,"bounding_box":{"x":283.8,"y":386.4,"width":44.5,"height":10.6}}},"text_order":5,"token_count":2,"style":null}
```

The dominant sequence transduction models are based on complex recurrent or convolutional neural networks that include an encoder and a decoder. The best performing models also connect the encoder and decoder through an attention mechanism. We propose a new simple network architecture, the Transformer, based solely on attention mechanisms, dispensing with recurrence and convolutions entirely. Experiments on two machine translation tasks show these models to be superior in quality while being more parallelizable and requiring significantly less time to train. Our model achieves 28.4 BLEU on the WMT 2014 English- to-German translation task, improving over the existing best results, including ensembles, by over 2 BLEU. On the WMT 2014 English-to-French translation task, our model establishes a new single-model state-of-the-art BLEU score of 41.8 after training for 3.5 days on eight GPUs, a small fraction of the training costs of the best models from the literature. We show that the Transformer generalizes well to other tasks by applying it successfully to English constituency parsing both with large and limited training data.
```bgraph-paragraph
{"id":"d895cd4c-17be-5518-b62c-4d8b9b443925","node_type":"Paragraph","location":{"semantic":{"path":"2.4.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","Abstract"]},"physical":{"page":1,"bounding_box":{"x":143.6,"y":413.5,"width":326.19998,"height":162.20001}}},"text_order":6,"token_count":275,"style":null}
```

∗ Equal contribution. Listing order is random. Jakob proposed replacing RNNs with self-attention and started the effort to evaluate this idea. Ashish, with Illia, designed and implemented the first Transformer models and has been crucially involved in every aspect of this work. Noam proposed scaled dot-product attention, multi-head attention and the parameter-free position representation and became the other person involved in nearly every detail. Niki designed, implemented, tuned and evaluated countless model variants in our original codebase and tensor2tensor. Llion also experimented with novel model variants, was responsible for our initial codebase, and efficient inference and visualizations. Lukasz and Aidan spent countless long days designing various parts of and implementing tensor2tensor, replacing our earlier codebase, greatly improving results and massively accelerating our research. † Work performed while at Google Brain. ‡ Work performed while at Google Research.
```bgraph-paragraph
{"id":"31ef44f2-92d2-5873-bab6-923aa006df71","node_type":"Paragraph","location":{"semantic":{"path":"2.4.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","Abstract"]},"physical":{"page":1,"bounding_box":{"x":108.0,"y":597.9,"width":396.3,"height":111.29999}}},"text_order":7,"token_count":243,"style":null}
```

31st Conference on Neural Information Processing Systems (NIPS 2017), Long Beach, CA, USA.
```bgraph-paragraph
{"id":"334e79c5-a8be-5664-8129-bd00c9b54347","node_type":"Paragraph","location":{"semantic":{"path":"2.4.3","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","Abstract"]},"physical":{"page":1,"bounding_box":{"x":108.0,"y":733.9,"width":352.1,"height":7.7}}},"text_order":8,"token_count":22,"style":null}
```

arXiv:1706.03762v7 [cs.CL] 2 Aug 2023
```bgraph-margin
{"id":"c7dca3bf-579d-5b28-93de-3489c7076946","node_type":"Margin","location":{"semantic":{"path":"2.4.4","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","Abstract"]},"physical":{"page":1,"bounding_box":{"x":32.0,"y":208.9,"width":0.0,"height":350.4}}},"text_order":9,"token_count":9,"style":null}
```

## 1 Introduction
```bgraph-section
{"id":"616ef26c-2148-5c81-b4cf-d2217949adb2","node_type":"Section","location":{"semantic":{"path":"2.5","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","1 Introduction"]},"physical":{"page":2,"bounding_box":{"x":108.0,"y":73.6,"width":82.8,"height":10.6}}},"text_order":10,"token_count":3,"style":null}
```

Recurrent neural networks, long short-term memory [ 13 ] and gated recurrent [ 7 ] neural networks in particular, have been firmly established as state of the art approaches in sequence modeling and transduction problems such as language modeling and machine translation [ 35 , 2 , 5 ]. Numerous efforts have since continued to push the boundaries of recurrent language models and encoder-decoder architectures [ 38 , 24 , 15 ]. Recurrent models typically factor computation along the symbol positions of the input and output sequences. Aligning the positions to steps in computation time, they generate a sequence of hidden states h , as a function of the previous hidden state h and the input for position t . This inherently t t − 1 sequential nature precludes parallelization within training examples, which becomes critical at longer sequence lengths, as memory constraints limit batching across examples. Recent work has achieved significant improvements in computational efficiency through factorization tricks [ 21 ] and conditional computation [ 32 ], while also improving model performance in case of the latter. The fundamental constraint of sequential computation, however, remains.
```bgraph-paragraph
{"id":"57aa00db-4bd8-54b5-8620-aab6cb9a355b","node_type":"Paragraph","location":{"semantic":{"path":"2.5.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","1 Introduction"]},"physical":{"page":2,"bounding_box":{"x":108.0,"y":99.2,"width":396.4,"height":145.8}}},"text_order":11,"token_count":294,"internal_refs":[{"text":"","source_page":2,"source_bbox":{"x":326.0,"y":98.9,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.hochreiter1997","page":10,"point":{"x":108.0,"y":339.0}}},{"text":"","source_page":2,"source_bbox":{"x":426.7,"y":99.2,"width":7.0,"height":8.6},"target":{"kind":"named","name":"cite.gruEval14","page":10,"point":{"x":112.0,"y":144.0}}},{"text":"","source_page":2,"source_bbox":{"x":419.3,"y":120.7,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.sutskever14","page":11,"point":{"x":108.0,"y":492.0}}},{"text":"","source_page":2,"source_bbox":{"x":434.8,"y":120.7,"width":7.0,"height":8.7},"target":{"kind":"named","name":"cite.bahdanau2014neural","page":9,"point":{"x":112.0,"y":641.0}}},{"text":"","source_page":2,"source_bbox":{"x":445.3,"y":120.7,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.cho2014learning","page":10,"point":{"x":112.0,"y":72.0}}},{"text":"","source_page":2,"source_bbox":{"x":163.7,"y":142.5,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}},{"text":"","source_page":2,"source_bbox":{"x":178.7,"y":142.5,"width":12.0,"height":8.7},"target":{"kind":"named","name":"cite.luong2015effective","page":10,"point":{"x":108.0,"y":699.0}}},{"text":"","source_page":2,"source_bbox":{"x":193.6,"y":142.5,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.jozefowicz2016exploring","page":10,"point":{"x":108.0,"y":411.0}}},{"text":"","source_page":2,"source_bbox":{"x":427.3,"y":213.5,"width":12.0,"height":8.7},"target":{"kind":"named","name":"cite.Kuchaiev2017Factorization","page":10,"point":{"x":108.0,"y":596.0}}},{"text":"","source_page":2,"source_bbox":{"x":163.6,"y":224.4,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.shazeer2017outrageously","page":11,"point":{"x":108.0,"y":345.0}}}],"style":null}
```

Attention mechanisms have become an integral part of compelling sequence modeling and transduc- tion models in various tasks, allowing modeling of dependencies without regard to their distance in the input or output sequences [ 2 , 19 ]. In all but a few cases [ 27 ], however, such attention mechanisms are used in conjunction with a recurrent network. In this work we propose the Transformer, a model architecture eschewing recurrence and instead relying entirely on an attention mechanism to draw global dependencies between input and output. The Transformer allows for significantly more parallelization and can reach a new state of the art in translation quality after being trained for as little as twelve hours on eight P100 GPUs.
```bgraph-paragraph
{"id":"530ea0be-1c7b-5773-9702-861dce264de0","node_type":"Paragraph","location":{"semantic":{"path":"2.5.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","1 Introduction"]},"physical":{"page":2,"bounding_box":{"x":107.6,"y":252.7,"width":398.1,"height":90.60002}}},"text_order":12,"token_count":179,"internal_refs":[{"text":"","source_page":2,"source_bbox":{"x":226.9,"y":273.5,"width":7.0,"height":8.7},"target":{"kind":"named","name":"cite.bahdanau2014neural","page":9,"point":{"x":112.0,"y":641.0}}},{"text":"","source_page":2,"source_bbox":{"x":236.7,"y":273.5,"width":12.0,"height":9.0},"target":{"kind":"named","name":"cite.structuredAttentionNetworks","page":10,"point":{"x":108.0,"y":545.0}}},{"text":"","source_page":2,"source_bbox":{"x":342.4,"y":273.5,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.decomposableAttnModel","page":11,"point":{"x":108.0,"y":151.0}}}],"style":null}
```

## 2 Background
```bgraph-section
{"id":"f0bab9cd-fae7-5b70-bce4-ac98bd65d09a","node_type":"Section","location":{"semantic":{"path":"2.6","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","2 Background"]},"physical":{"page":2,"bounding_box":{"x":108.0,"y":363.9,"width":80.8,"height":10.6}}},"text_order":13,"token_count":3,"style":null}
```

The goal of reducing sequential computation also forms the foundation of the Extended Neural GPU [ 16 ], ByteNet [ 18 ] and ConvS2S [ 9 ], all of which use convolutional neural networks as basic building block, computing hidden representations in parallel for all input and output positions. In these models, the number of operations required to relate signals from two arbitrary input or output positions grows in the distance between positions, linearly for ConvS2S and logarithmically for ByteNet. This makes it more difficult to learn dependencies between distant positions [ 12 ]. In the Transformer this is reduced to a constant number of operations, albeit at the cost of reduced effective resolution due to averaging attention-weighted positions, an effect we counteract with Multi-Head Attention as described in section 3.2 .
```bgraph-paragraph
{"id":"4841913f-d743-51a8-a597-d4bc3a521ef8","node_type":"Paragraph","location":{"semantic":{"path":"2.6.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","2 Background"]},"physical":{"page":2,"bounding_box":{"x":107.7,"y":390.2,"width":397.5,"height":96.0}}},"text_order":14,"token_count":203,"internal_refs":[{"text":"","source_page":2,"source_bbox":{"x":110.3,"y":400.1,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.extendedngpu","page":10,"point":{"x":108.0,"y":442.0}}},{"text":"","source_page":2,"source_bbox":{"x":166.4,"y":400.1,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.NalBytenet2017","page":10,"point":{"x":108.0,"y":504.0}}},{"text":"","source_page":2,"source_bbox":{"x":240.5,"y":400.1,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}},{"text":"","source_page":2,"source_bbox":{"x":377.2,"y":443.8,"width":12.0,"height":8.7},"target":{"kind":"named","name":"cite.hochreiter2001gradient","page":10,"point":{"x":108.0,"y":308.0}}},{"text":"","source_page":2,"source_bbox":{"x":188.6,"y":476.5,"width":14.4,"height":8.8},"target":{"kind":"named","name":"subsection.3.2","page":2,"point":{"x":108.0,"y":675.0}}}],"style":null}
```

Self-attention, sometimes called intra-attention is an attention mechanism relating different positions of a single sequence in order to compute a representation of the sequence. Self-attention has been used successfully in a variety of tasks including reading comprehension, abstractive summarization, textual entailment and learning task-independent sentence representations [ 4 , 27 , 28 , 22 ]. End-to-end memory networks are based on a recurrent attention mechanism instead of sequence- aligned recurrence and have been shown to perform well on simple-language question answering and language modeling tasks [ 34 ]. To the best of our knowledge, however, the Transformer is the first transduction model relying entirely on self-attention to compute representations of its input and output without using sequence- aligned RNNs or convolution. In the following sections, we will describe the Transformer, motivate self-attention and discuss its advantages over models such as [ 17 , 18 ] and [ 9 ].
```bgraph-paragraph
{"id":"14117a4a-bffc-53b5-a6d4-4c42f3809ac6","node_type":"Paragraph","location":{"semantic":{"path":"2.6.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","2 Background"]},"physical":{"page":2,"bounding_box":{"x":107.7,"y":493.9,"width":398.0,"height":128.80002}}},"text_order":15,"token_count":247,"internal_refs":[{"text":"","source_page":2,"source_bbox":{"x":406.5,"y":525.6,"width":7.0,"height":8.7},"target":{"kind":"named","name":"cite.cheng2016long","page":9,"point":{"x":112.0,"y":699.0}}},{"text":"","source_page":2,"source_bbox":{"x":416.5,"y":525.6,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.decomposableAttnModel","page":11,"point":{"x":108.0,"y":151.0}}},{"text":"","source_page":2,"source_bbox":{"x":431.4,"y":525.6,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.paulus2017deep","page":11,"point":{"x":108.0,"y":186.0}}},{"text":"","source_page":2,"source_bbox":{"x":446.3,"y":525.6,"width":12.0,"height":8.7},"target":{"kind":"named","name":"cite.lin2017structured","page":10,"point":{"x":108.0,"y":626.0}}},{"text":"","source_page":2,"source_bbox":{"x":211.3,"y":563.8,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.sukhbaatar2015","page":11,"point":{"x":108.0,"y":436.0}}},{"text":"","source_page":2,"source_bbox":{"x":354.7,"y":612.9,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.neural_gpu","page":10,"point":{"x":108.0,"y":473.0}}},{"text":"","source_page":2,"source_bbox":{"x":369.6,"y":612.9,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.NalBytenet2017","page":10,"point":{"x":108.0,"y":504.0}}},{"text":"","source_page":2,"source_bbox":{"x":405.6,"y":612.9,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}}],"style":null}
```

## 3 Model Architecture
```bgraph-section
{"id":"19f58d2b-8a54-554f-a0f7-3ac90c0e77f1","node_type":"Section","location":{"semantic":{"path":"2.7","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture"]},"physical":{"page":2,"bounding_box":{"x":108.0,"y":643.3,"width":118.1,"height":10.6}}},"text_order":16,"token_count":5,"style":null}
```

Most competitive neural sequence transduction models have an encoder-decoder structure [ 5 , 2 , 35 ]. Here, the encoder maps an input sequence of symbol representations ( x , ..., x ) to a sequence 1 n of continuous representations z = ( z , ..., z ) . Given z , the decoder then generates an output 1 n sequence ( y , ..., y ) of symbols one element at a time. At each step the model is auto-regressive 1 m [ 10 ], consuming the previously generated symbols as additional input when generating the next.
```bgraph-paragraph
{"id":"257ca1e5-db6c-5ede-8590-d6bb76f19a94","node_type":"Paragraph","location":{"semantic":{"path":"2.7.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture"]},"physical":{"page":2,"bounding_box":{"x":108.0,"y":669.7,"width":397.8,"height":52.299988}}},"text_order":17,"token_count":128,"internal_refs":[{"text":"","source_page":2,"source_bbox":{"x":469.1,"y":668.6,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.cho2014learning","page":10,"point":{"x":112.0,"y":72.0}}},{"text":"","source_page":2,"source_bbox":{"x":479.0,"y":668.6,"width":7.0,"height":8.7},"target":{"kind":"named","name":"cite.bahdanau2014neural","page":9,"point":{"x":112.0,"y":641.0}}},{"text":"","source_page":2,"source_bbox":{"x":489.0,"y":668.6,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.sutskever14","page":11,"point":{"x":108.0,"y":492.0}}},{"text":"","source_page":2,"source_bbox":{"x":110.3,"y":712.2,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.graves2013generating","page":10,"point":{"x":108.0,"y":236.0}}}],"style":null}
```

2
```bgraph-paragraph
{"id":"27da1998-b33a-50a9-b040-6111af98b228","node_type":"Paragraph","location":{"semantic":{"path":"2.7.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture"]},"physical":{"page":2,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":18,"token_count":1,"style":null}
```

Figure 1: The Transformer - model architecture.
```bgraph-paragraph
{"id":"aadc9fdd-a8a3-58ac-b6b3-b8c01178dbe1","node_type":"Paragraph","location":{"semantic":{"path":"2.7.3","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture"]},"physical":{"page":3,"bounding_box":{"x":210.0,"y":405.6,"width":192.0,"height":8.7}}},"text_order":19,"token_count":11,"style":null}
```

The Transformer follows this overall architecture using stacked self-attention and point-wise, fully connected layers for both the encoder and decoder, shown in the left and right halves of Figure 1 , respectively.
```bgraph-paragraph
{"id":"b765045f-9fad-594e-a772-71661b7d43c8","node_type":"Paragraph","location":{"semantic":{"path":"2.7.4","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture"]},"physical":{"page":3,"bounding_box":{"x":107.7,"y":436.9,"width":397.5,"height":31.300018}}},"text_order":20,"token_count":53,"internal_refs":[{"text":"","source_page":3,"source_bbox":{"x":496.6,"y":447.5,"width":7.1,"height":10.9},"target":{"kind":"named","name":"figure.1","page":2,"point":{"x":249.0,"y":402.0}}}],"style":null}
```

### 3.1 Encoder and Decoder Stacks
```bgraph-section
{"id":"ee146ebc-b80c-570f-99c6-64f84d8eab4c","node_type":"Section","location":{"semantic":{"path":"2.7.5","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.1 Encoder and Decoder Stacks"]},"physical":{"page":3,"bounding_box":{"x":108.0,"y":483.8,"width":145.0,"height":8.700012}}},"text_order":21,"token_count":7,"style":null}
```

Encoder: The encoder is composed of a stack of N = 6 identical layers. Each layer has two sub-layers. The first is a multi-head self-attention mechanism, and the second is a simple, position- wise fully connected feed-forward network. We employ a residual connection [ 11 ] around each of the two sub-layers, followed by layer normalization [ 1 ]. That is, the output of each sub-layer is LayerNorm( x + Sublayer( x )) , where Sublayer( x ) is the function implemented by the sub-layer itself. To facilitate these residual connections, all sub-layers in the model, as well as the embedding layers, produce outputs of dimension d = 512 . model
```bgraph-paragraph
{"id":"310a29e2-4b29-5469-8ecd-ba101bd5246d","node_type":"Paragraph","location":{"semantic":{"path":"2.7.5.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.1 Encoder and Decoder Stacks"]},"physical":{"page":3,"bounding_box":{"x":107.6,"y":503.0,"width":398.1,"height":75.89996}}},"text_order":22,"token_count":156,"internal_refs":[{"text":"","source_page":3,"source_bbox":{"x":427.1,"y":524.7,"width":12.0,"height":8.7},"target":{"kind":"named","name":"cite.he2016deep","page":10,"point":{"x":108.0,"y":267.0}}},{"text":"","source_page":3,"source_bbox":{"x":327.1,"y":535.6,"width":7.0,"height":8.7},"target":{"kind":"named","name":"cite.layernorm2016","page":9,"point":{"x":112.0,"y":612.0}}}],"style":null}
```

Decoder: The decoder is also composed of a stack of N = 6 identical layers. In addition to the two sub-layers in each encoder layer, the decoder inserts a third sub-layer, which performs multi-head attention over the output of the encoder stack. Similar to the encoder, we employ residual connections around each of the sub-layers, followed by layer normalization. We also modify the self-attention sub-layer in the decoder stack to prevent positions from attending to subsequent positions. This masking, combined with fact that the output embeddings are offset by one position, ensures that the predictions for position i can depend only on the known outputs at positions less than i .
```bgraph-paragraph
{"id":"8f8a352b-237d-5be2-be50-54fe38466bfd","node_type":"Paragraph","location":{"semantic":{"path":"2.7.5.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.1 Encoder and Decoder Stacks"]},"physical":{"page":3,"bounding_box":{"x":108.0,"y":592.3,"width":396.0,"height":74.30005}}},"text_order":23,"token_count":168,"style":null}
```

### 3.2 Attention
```bgraph-section
{"id":"e869cda0-9068-51df-8963-ea8f2f31533f","node_type":"Section","location":{"semantic":{"path":"2.7.6","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention"]},"physical":{"page":3,"bounding_box":{"x":108.0,"y":682.3,"width":62.799988,"height":8.700012}}},"text_order":24,"token_count":3,"style":null}
```

An attention function can be described as mapping a query and a set of key-value pairs to an output, where the query, keys, values, and output are all vectors. The output is computed as a weighted sum
```bgraph-paragraph
{"id":"470489dc-e4cf-597f-b094-79fe6c27dbae","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention"]},"physical":{"page":3,"bounding_box":{"x":107.6,"y":702.4,"width":397.6,"height":19.599976}}},"text_order":25,"token_count":49,"style":null}
```

3
```bgraph-paragraph
{"id":"202ad850-2e3b-5e98-a785-b260f2b2b7ab","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention"]},"physical":{"page":3,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":26,"token_count":1,"style":null}
```

Scaled Dot-Product Attention Multi-Head Attention
```bgraph-paragraph
{"id":"0cff8dd7-c9b2-5738-9d74-b5e6821c84eb","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention"]},"physical":{"page":4,"bounding_box":{"x":147.8,"y":72.0,"width":302.40002,"height":8.699997}}},"text_order":27,"token_count":12,"style":null}
```

Figure 2: (left) Scaled Dot-Product Attention. (right) Multi-Head Attention consists of several attention layers running in parallel.
```bgraph-paragraph
{"id":"39325d61-cb63-5357-9082-29e82634d6d0","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.4","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention"]},"physical":{"page":4,"bounding_box":{"x":108.0,"y":274.6,"width":396.0,"height":20.300018}}},"text_order":28,"token_count":32,"style":null}
```

of the values, where the weight assigned to each value is computed by a compatibility function of the query with the corresponding key.
```bgraph-paragraph
{"id":"b475433c-af18-5274-b499-1fec29063d3f","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.5","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention"]},"physical":{"page":4,"bounding_box":{"x":108.0,"y":318.0,"width":396.0,"height":19.600006}}},"text_order":29,"token_count":33,"style":null}
```

#### 3.2.1 Scaled Dot-Product Attention
```bgraph-section
{"id":"c418570d-e656-5f82-b288-cb44fd5c05df","node_type":"Section","location":{"semantic":{"path":"2.7.6.6","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.1 Scaled Dot-Product Attention"]},"physical":{"page":4,"bounding_box":{"x":108.0,"y":351.9,"width":155.9,"height":8.7}}},"text_order":30,"token_count":8,"style":null}
```

We call our particular attention "Scaled Dot-Product Attention" (Figure 2 ). The input consists of queries and keys of dimension d , and values of dimension d . We compute the dot products of the k √ v query with all keys, divide each by d , and apply a softmax function to obtain the weights on the k values.
```bgraph-paragraph
{"id":"9921a543-4d73-56b5-87b7-e1aefd6cf39f","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.6.1","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.1 Scaled Dot-Product Attention"]},"physical":{"page":4,"bounding_box":{"x":107.5,"y":369.9,"width":396.5,"height":42.100006}}},"text_order":31,"token_count":76,"internal_refs":[{"text":"","source_page":4,"source_bbox":{"x":402.0,"y":369.6,"width":7.1,"height":10.9},"target":{"kind":"named","name":"figure.2","page":3,"point":{"x":150.0,"y":272.0}}}],"style":null}
```

In practice, we compute the attention function on a set of queries simultaneously, packed together into a matrix Q . The keys and values are also packed together into matrices K and V . We compute the matrix of outputs as:
```bgraph-paragraph
{"id":"72aae5c2-a0d4-5aa6-8b1e-3be3bc2b98b5","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.6.2","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.1 Scaled Dot-Product Attention"]},"physical":{"page":4,"bounding_box":{"x":108.0,"y":419.0,"width":396.2,"height":31.300018}}},"text_order":32,"token_count":55,"style":null}
```

QK T Attention( Q, K, V ) = softmax( √ ) V (1) d k
```bgraph-paragraph
{"id":"cd04fb9b-e1ae-565e-84c8-3f098902b611","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.6.3","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.1 Scaled Dot-Product Attention"]},"physical":{"page":4,"bounding_box":{"x":220.0,"y":465.8,"width":284.7,"height":25.00003}}},"text_order":33,"token_count":14,"style":null}
```

The two most commonly used attention functions are additive attention [ 2 ], and dot-product (multi- plicative) attention. Dot-product attention is identical to our algorithm, except for the scaling factor of √ 1 . Additive attention computes the compatibility function using a feed-forward network with d k a single hidden layer. While the two are similar in theoretical complexity, dot-product attention is much faster and more space-efficient in practice, since it can be implemented using highly optimized matrix multiplication code.
```bgraph-paragraph
{"id":"b92726be-a879-53fd-bd94-993a8d3994bc","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.6.4","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.1 Scaled Dot-Product Attention"]},"physical":{"page":4,"bounding_box":{"x":107.7,"y":499.6,"width":398.0,"height":66.30002}}},"text_order":34,"token_count":133,"internal_refs":[{"text":"","source_page":4,"source_bbox":{"x":397.4,"y":499.3,"width":7.0,"height":8.7},"target":{"kind":"named","name":"cite.bahdanau2014neural","page":9,"point":{"x":112.0,"y":641.0}}}],"style":null}
```

While for small values of d the two mechanisms perform similarly, additive attention outperforms k dot product attention without scaling for larger values of d [ 3 ]. We suspect that for large values of k d , the dot products grow large in magnitude, pushing the softmax function into regions where it has k extremely small gradients 4 . To counteract this effect, we scale the dot products by √ 1 . d k
```bgraph-paragraph
{"id":"eb2d306b-8427-55dd-9b37-1432826acaac","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.6.5","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.1 Scaled Dot-Product Attention"]},"physical":{"page":4,"bounding_box":{"x":107.5,"y":572.8,"width":396.5,"height":46.200012}}},"text_order":35,"token_count":104,"internal_refs":[{"text":"","source_page":4,"source_bbox":{"x":351.3,"y":583.4,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.DBLP:journals/corr/BritzGLL17","page":9,"point":{"x":112.0,"y":670.0}}},{"text":"","source_page":4,"source_bbox":{"x":212.3,"y":603.6,"width":6.0,"height":15.8},"target":{"kind":"named","name":"Hfootnote.1","page":3,"point":{"x":124.0,"y":699.0}}}],"style":null}
```

#### 3.2.2 Multi-Head Attention
```bgraph-section
{"id":"78a95477-ad64-5837-b987-28d0c73fb8d2","node_type":"Section","location":{"semantic":{"path":"2.7.6.7","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":4,"bounding_box":{"x":108.0,"y":631.6,"width":122.6,"height":8.7}}},"text_order":36,"token_count":6,"style":null}
```

Instead of performing a single attention function with d -dimensional keys, values and queries, model we found it beneficial to linearly project the queries, keys and values h times with different, learned linear projections to d , d and d dimensions, respectively. On each of these projected versions of k k v queries, keys and values we then perform the attention function in parallel, yielding d -dimensional v
```bgraph-paragraph
{"id":"a5a6c53e-5977-5836-bea1-1b24bb6428e4","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.7.1","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":4,"bounding_box":{"x":107.6,"y":649.6,"width":397.69998,"height":42.80005}}},"text_order":37,"token_count":103,"style":null}
```

4 To illustrate why the dot products get large, assume that the components of q and k are independent random d variables with mean 0 and variance 1 . Then their dot product, q · k = ∑ k q k , has mean 0 and variance d . i =1 i i k
```bgraph-paragraph
{"id":"8727070d-9206-5f0e-a5f1-dade43eb4377","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.7.2","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":4,"bounding_box":{"x":107.8,"y":701.1,"width":396.2,"height":22.600037}}},"text_order":38,"token_count":62,"style":null}
```

4
```bgraph-paragraph
{"id":"fc23deff-9c46-52c9-8b2f-998ec54bd9f5","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.7.3","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":4,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":39,"token_count":1,"style":null}
```

output values. These are concatenated and once again projected, resulting in the final values, as depicted in Figure 2 .
```bgraph-paragraph
{"id":"fecfb28c-bfbe-5c38-a49d-ca6aca515a4e","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.7.4","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":5,"bounding_box":{"x":108.0,"y":74.5,"width":396.0,"height":20.399994}}},"text_order":40,"token_count":30,"internal_refs":[{"text":"","source_page":5,"source_bbox":{"x":182.0,"y":85.1,"width":7.0,"height":10.9},"target":{"kind":"named","name":"figure.2","page":3,"point":{"x":150.0,"y":272.0}}}],"style":null}
```

Multi-head attention allows the model to jointly attend to information from different representation subspaces at different positions. With a single attention head, averaging inhibits this.
```bgraph-paragraph
{"id":"f92e5cff-25fa-5b11-82ac-a16daddba219","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.7.5","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":5,"bounding_box":{"x":108.0,"y":101.8,"width":396.0,"height":20.399994}}},"text_order":41,"token_count":47,"style":null}
```

MultiHead( Q, K, V ) = Concat(head , ..., head ) W O 1 h Q where head = Attention( QW , KW K , V W V ) i i i i
```bgraph-paragraph
{"id":"63ba379a-c417-592e-93c9-0d0834276120","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.7.6","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":5,"bounding_box":{"x":186.9,"y":146.7,"width":238.20001,"height":29.399994}}},"text_order":42,"token_count":27,"style":null}
```

Q Where the projections are parameter matrices W ∈ d model × d k , W K ∈ d model × d k , W V ∈ d model × d v R R R i i i and W O ∈ hd v × d model . R In this work we employ h = 8 parallel attention layers, or heads. For each of these we use d = d = d /h = 64 . Due to the reduced dimension of each head, the total computational cost k v model is similar to that of single-head attention with full dimensionality.
```bgraph-paragraph
{"id":"cd4c99b2-4bd8-5d59-8f9c-16719792d264","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.7.7","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.2 Multi-Head Attention"]},"physical":{"page":5,"bounding_box":{"x":107.5,"y":203.9,"width":396.5,"height":61.80002}}},"text_order":43,"token_count":120,"style":null}
```

#### 3.2.3 Applications of Attention in our Model
```bgraph-section
{"id":"3c1c85d9-dc57-52a9-8b13-e06e997f1ae3","node_type":"Section","location":{"semantic":{"path":"2.7.6.8","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.3 Applications of Attention in our Model"]},"physical":{"page":5,"bounding_box":{"x":108.0,"y":281.1,"width":194.9,"height":8.7}}},"text_order":44,"token_count":11,"style":null}
```

The Transformer uses multi-head attention in three different ways:
```bgraph-paragraph
{"id":"79e612af-bce8-573b-a77f-9f3bca40936d","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.8.1","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.3 Applications of Attention in our Model"]},"physical":{"page":5,"bounding_box":{"x":107.7,"y":300.3,"width":264.9,"height":8.7}}},"text_order":45,"token_count":16,"style":null}
```

• In "encoder-decoder attention" layers, the queries come from the previous decoder layer, and the memory keys and values come from the output of the encoder. This allows every position in the decoder to attend over all positions in the input sequence. This mimics the typical encoder-decoder attention mechanisms in sequence-to-sequence models such as [ 38 , 2 , 9 ].
```bgraph-paragraph
{"id":"a68275d9-87ad-50ff-8f44-2388ce266ccb","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.8.2","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.3 Applications of Attention in our Model"]},"physical":{"page":5,"bounding_box":{"x":135.4,"y":320.4,"width":369.9,"height":53.00003}}},"text_order":46,"token_count":93,"internal_refs":[{"text":"","source_page":5,"source_bbox":{"x":146.2,"y":363.7,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}},{"text":"","source_page":5,"source_bbox":{"x":161.1,"y":363.7,"width":7.0,"height":8.7},"target":{"kind":"named","name":"cite.bahdanau2014neural","page":9,"point":{"x":112.0,"y":641.0}}},{"text":"","source_page":5,"source_bbox":{"x":171.1,"y":363.7,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}}],"style":null}
```

• The encoder contains self-attention layers. In a self-attention layer all of the keys, values and queries come from the same place, in this case, the output of the previous layer in the encoder. Each position in the encoder can attend to all positions in the previous layer of the encoder.
```bgraph-paragraph
{"id":"dfa08b58-1214-52b8-b11a-c99a63e00f7d","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.8.3","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.3 Applications of Attention in our Model"]},"physical":{"page":5,"bounding_box":{"x":135.4,"y":379.8,"width":368.6,"height":42.200012}}},"text_order":47,"token_count":71,"style":null}
```

• Similarly, self-attention layers in the decoder allow each position in the decoder to attend to all positions in the decoder up to and including that position. We need to prevent leftward information flow in the decoder to preserve the auto-regressive property. We implement this inside of scaled dot-product attention by masking out (setting to −∞ ) all values in the input of the softmax which correspond to illegal connections. See Figure 2 .
```bgraph-paragraph
{"id":"30e830ab-2aa7-5c7b-9fec-a86bab391fd7","node_type":"Paragraph","location":{"semantic":{"path":"2.7.6.8.4","depth":5,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.2 Attention","3.2.3 Applications of Attention in our Model"]},"physical":{"page":5,"bounding_box":{"x":135.4,"y":429.1,"width":368.6,"height":52.399994}}},"text_order":48,"token_count":109,"internal_refs":[{"text":"","source_page":5,"source_bbox":{"x":412.5,"y":471.7,"width":7.0,"height":10.9},"target":{"kind":"named","name":"figure.2","page":3,"point":{"x":150.0,"y":272.0}}}],"style":null}
```

### 3.3 Position-wise Feed-Forward Networks
```bgraph-section
{"id":"ee7b45c9-1287-5d39-853a-7a4b10acd1cc","node_type":"Section","location":{"semantic":{"path":"2.7.7","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.3 Position-wise Feed-Forward Networks"]},"physical":{"page":5,"bounding_box":{"x":108.0,"y":498.2,"width":184.9,"height":8.700012}}},"text_order":49,"token_count":9,"style":null}
```

In addition to attention sub-layers, each of the layers in our encoder and decoder contains a fully connected feed-forward network, which is applied to each position separately and identically. This consists of two linear transformations with a ReLU activation in between.
```bgraph-paragraph
{"id":"2289a985-19fe-5da9-aa05-30411fda140b","node_type":"Paragraph","location":{"semantic":{"path":"2.7.7.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.3 Position-wise Feed-Forward Networks"]},"physical":{"page":5,"bounding_box":{"x":108.0,"y":517.9,"width":396.3,"height":31.299988}}},"text_order":50,"token_count":66,"style":null}
```

FFN( x ) = max(0 , xW + b ) W + b (2) 1 1 2 2
```bgraph-paragraph
{"id":"a5a8016c-3e71-5e6f-bdf3-26060d2eb23d","node_type":"Paragraph","location":{"semantic":{"path":"2.7.7.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.3 Position-wise Feed-Forward Networks"]},"physical":{"page":5,"bounding_box":{"x":226.9,"y":569.1,"width":277.80002,"height":9.400024}}},"text_order":51,"token_count":15,"style":null}
```

While the linear transformations are the same across different positions, they use different parameters from layer to layer. Another way of describing this is as two convolutions with kernel size 1. The dimensionality of input and output is d = 512 , and the inner-layer has dimensionality model d = 2048 . ff
```bgraph-paragraph
{"id":"80bc3270-3729-5f15-a13a-608b9c455e55","node_type":"Paragraph","location":{"semantic":{"path":"2.7.7.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.3 Position-wise Feed-Forward Networks"]},"physical":{"page":5,"bounding_box":{"x":107.5,"y":591.0,"width":398.2,"height":42.100037}}},"text_order":52,"token_count":75,"style":null}
```

### 3.4 Embeddings and Softmax
```bgraph-section
{"id":"27edeb08-37d8-5747-b385-f6f48832c5d8","node_type":"Section","location":{"semantic":{"path":"2.7.8","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.4 Embeddings and Softmax"]},"physical":{"page":5,"bounding_box":{"x":108.0,"y":649.1,"width":132.0,"height":8.7}}},"text_order":53,"token_count":6,"style":null}
```

Similarly to other sequence transduction models, we use learned embeddings to convert the input tokens and output tokens to vectors of dimension d . We also use the usual learned linear transfor- model mation and softmax function to convert the decoder output to predicted next-token probabilities. In our model, we share the same weight matrix between the two embedding layers and the pre-softmax √ linear transformation, similar to [ 30 ]. In the embedding layers, we multiply those weights by d . model
```bgraph-paragraph
{"id":"20ce4a20-bf98-5439-9979-05bc5c6fa047","node_type":"Paragraph","location":{"semantic":{"path":"2.7.8.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.4 Embeddings and Softmax"]},"physical":{"page":5,"bounding_box":{"x":108.0,"y":668.9,"width":397.69998,"height":53.899963}}},"text_order":54,"token_count":123,"internal_refs":[{"text":"","source_page":5,"source_bbox":{"x":236.6,"y":712.2,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.press2016using","page":11,"point":{"x":108.0,"y":276.0}}}],"style":null}
```

5
```bgraph-paragraph
{"id":"9541851c-67c9-5b66-b9cc-ba670acf1f10","node_type":"Paragraph","location":{"semantic":{"path":"2.7.8.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.4 Embeddings and Softmax"]},"physical":{"page":5,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":55,"token_count":1,"style":null}
```

Table 1: Maximum path lengths, per-layer complexity and minimum number of sequential operations for different layer types. n is the sequence length, d is the representation dimension, k is the kernel size of convolutions and r the size of the neighborhood in restricted self-attention.
```bgraph-paragraph
{"id":"7d1575c7-789a-55be-b821-68b1fdc574bd","node_type":"Paragraph","location":{"semantic":{"path":"2.7.8.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.4 Embeddings and Softmax"]},"physical":{"page":6,"bounding_box":{"x":107.7,"y":72.0,"width":396.3,"height":30.599998}}},"text_order":56,"token_count":69,"style":null}
```

Layer Type Complexity per Layer Sequential Maximum Path Length Operations Self-Attention O ( n 2 · d ) O (1) O (1) Recurrent O ( n · d 2 ) O ( n ) O ( n ) Convolutional O ( k · n · d 2 ) O (1) O ( log ( n )) k Self-Attention (restricted) O ( r · n · d ) O (1) O ( n/r )
```bgraph-paragraph
{"id":"fd1a70d8-063a-58cc-b63a-4d9cd7ca3ea1","node_type":"Paragraph","location":{"semantic":{"path":"2.7.8.4","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.4 Embeddings and Softmax"]},"physical":{"page":6,"bounding_box":{"x":124.5,"y":117.5,"width":363.0,"height":65.899994}}},"text_order":57,"token_count":87,"style":null}
```

### 3.5 Positional Encoding
```bgraph-section
{"id":"230793e7-2dc8-52a8-a858-9bcdaf914210","node_type":"Section","location":{"semantic":{"path":"2.7.9","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.5 Positional Encoding"]},"physical":{"page":6,"bounding_box":{"x":108.0,"y":214.6,"width":107.2,"height":8.699997}}},"text_order":58,"token_count":5,"style":null}
```

Since our model contains no recurrence and no convolution, in order for the model to make use of the order of the sequence, we must inject some information about the relative or absolute position of the tokens in the sequence. To this end, we add "positional encodings" to the input embeddings at the bottoms of the encoder and decoder stacks. The positional encodings have the same dimension d model as the embeddings, so that the two can be summed. There are many choices of positional encodings, learned and fixed [ 9 ].
```bgraph-paragraph
{"id":"6e69224b-69c9-59df-8a0f-aaacedf55212","node_type":"Paragraph","location":{"semantic":{"path":"2.7.9.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.5 Positional Encoding"]},"physical":{"page":6,"bounding_box":{"x":108.0,"y":234.7,"width":397.2,"height":63.200027}}},"text_order":59,"token_count":128,"internal_refs":[{"text":"","source_page":6,"source_bbox":{"x":181.3,"y":288.2,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}}],"style":null}
```

In this work, we use sine and cosine functions of different frequencies:
```bgraph-paragraph
{"id":"e3d6b76f-6702-52f3-8a4d-2da132c17f1b","node_type":"Paragraph","location":{"semantic":{"path":"2.7.9.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.5 Positional Encoding"]},"physical":{"page":6,"bounding_box":{"x":108.0,"y":305.6,"width":281.9,"height":8.7}}},"text_order":60,"token_count":18,"style":null}
```

PE = sin ( pos/ 10000 2 i/d model ) ( pos, 2 i ) PE = cos ( pos/ 10000 2 i/d model ) ( pos, 2 i +1)
```bgraph-paragraph
{"id":"6cfa4ba4-1f72-5c37-939c-6b89a7422e3a","node_type":"Paragraph","location":{"semantic":{"path":"2.7.9.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.5 Positional Encoding"]},"physical":{"page":6,"bounding_box":{"x":225.7,"y":337.0,"width":160.59999,"height":28.5}}},"text_order":61,"token_count":30,"style":null}
```

where pos is the position and i is the dimension. That is, each dimension of the positional encoding corresponds to a sinusoid. The wavelengths form a geometric progression from 2 π to 10000 · 2 π . We chose this function because we hypothesized it would allow the model to easily learn to attend by relative positions, since for any fixed offset k , PE can be represented as a linear function of pos + k PE . pos We also experimented with using learned positional embeddings [ 9 ] instead, and found that the two versions produced nearly identical results (see Table 3 row (E)). We chose the sinusoidal version because it may allow the model to extrapolate to sequence lengths longer than the ones encountered during training.
```bgraph-paragraph
{"id":"38bcdf5e-2c37-57b6-8c33-db930b35d660","node_type":"Paragraph","location":{"semantic":{"path":"2.7.9.4","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","3 Model Architecture","3.5 Positional Encoding"]},"physical":{"page":6,"bounding_box":{"x":107.5,"y":378.2,"width":396.8,"height":101.600006}}},"text_order":62,"token_count":181,"internal_refs":[{"text":"","source_page":6,"source_bbox":{"x":369.3,"y":437.4,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}},{"text":"","source_page":6,"source_bbox":{"x":323.8,"y":448.3,"width":7.1,"height":10.9},"target":{"kind":"named","name":"table.3","page":8,"point":{"x":142.0,"y":68.0}}}],"style":null}
```

## 4 Why Self-Attention
```bgraph-section
{"id":"d167f628-efa0-586f-bfa2-274e7df9469d","node_type":"Section","location":{"semantic":{"path":"2.8","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","4 Why Self-Attention"]},"physical":{"page":6,"bounding_box":{"x":108.0,"y":497.7,"width":117.0,"height":10.6}}},"text_order":63,"token_count":5,"style":null}
```

In this section we compare various aspects of self-attention layers to the recurrent and convolu- tional layers commonly used for mapping one variable-length sequence of symbol representations ( x , ..., x ) to another sequence of equal length ( z , ..., z ) , with x , z ∈ d , such as a hidden 1 n 1 n i i R layer in a typical sequence transduction encoder or decoder. Motivating our use of self-attention we consider three desiderata.
```bgraph-paragraph
{"id":"7610a2b2-a395-554c-ade9-da16ba815e71","node_type":"Paragraph","location":{"semantic":{"path":"2.8.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","4 Why Self-Attention"]},"physical":{"page":6,"bounding_box":{"x":106.8,"y":521.6,"width":398.90002,"height":53.00006}}},"text_order":64,"token_count":108,"style":null}
```

One is the total computational complexity per layer. Another is the amount of computation that can be parallelized, as measured by the minimum number of sequential operations required. The third is the path length between long-range dependencies in the network. Learning long-range dependencies is a key challenge in many sequence transduction tasks. One key factor affecting the ability to learn such dependencies is the length of the paths forward and backward signals have to traverse in the network. The shorter these paths between any combination of positions in the input and output sequences, the easier it is to learn long-range dependencies [ 12 ]. Hence we also compare the maximum path length between any two input and output positions in networks composed of the different layer types.
```bgraph-paragraph
{"id":"13366d65-70ff-555f-b8dc-e3c3cf002029","node_type":"Paragraph","location":{"semantic":{"path":"2.8.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","4 Why Self-Attention"]},"physical":{"page":6,"bounding_box":{"x":107.7,"y":582.3,"width":396.3,"height":101.5}}},"text_order":65,"token_count":194,"internal_refs":[{"text":"","source_page":6,"source_bbox":{"x":390.1,"y":652.2,"width":12.0,"height":8.7},"target":{"kind":"named","name":"cite.hochreiter2001gradient","page":10,"point":{"x":108.0,"y":308.0}}}],"style":null}
```

As noted in Table 1 , a self-attention layer connects all positions with a constant number of sequentially executed operations, whereas a recurrent layer requires O ( n ) sequential operations. In terms of computational complexity, self-attention layers are faster than recurrent layers when the sequence
```bgraph-paragraph
{"id":"22f0c601-83b3-510a-9035-fc2b7f998a21","node_type":"Paragraph","location":{"semantic":{"path":"2.8.3","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","4 Why Self-Attention"]},"physical":{"page":6,"bounding_box":{"x":107.6,"y":691.5,"width":396.69998,"height":30.599976}}},"text_order":66,"token_count":75,"internal_refs":[{"text":"","source_page":6,"source_bbox":{"x":176.7,"y":690.4,"width":6.9,"height":10.9},"target":{"kind":"named","name":"table.1","page":5,"point":{"x":141.0,"y":68.0}}}],"style":null}
```

6
```bgraph-paragraph
{"id":"3d953912-fb32-5de0-a3e3-32ae6b860d03","node_type":"Paragraph","location":{"semantic":{"path":"2.8.4","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","4 Why Self-Attention"]},"physical":{"page":6,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":67,"token_count":1,"style":null}
```

length n is smaller than the representation dimensionality d , which is most often the case with sentence representations used by state-of-the-art models in machine translations, such as word-piece [ 38 ] and byte-pair [ 31 ] representations. To improve computational performance for tasks involving very long sequences, self-attention could be restricted to considering only a neighborhood of size r in the input sequence centered around the respective output position. This would increase the maximum path length to O ( n/r ) . We plan to investigate this approach further in future work. A single convolutional layer with kernel width k < n does not connect all pairs of input and output positions. Doing so requires a stack of O ( n/k ) convolutional layers in the case of contiguous kernels, or O ( log ( n )) in the case of dilated convolutions [ 18 ], increasing the length of the longest paths k between any two positions in the network. Convolutional layers are generally more expensive than recurrent layers, by a factor of k . Separable convolutions [ 6 ], however, decrease the complexity considerably, to O ( k · n · d + n · d 2 ) . Even with k = n , however, the complexity of a separable convolution is equal to the combination of a self-attention layer and a point-wise feed-forward layer, the approach we take in our model. As side benefit, self-attention could yield more interpretable models. We inspect attention distributions from our models and present and discuss examples in the appendix. Not only do individual attention heads clearly learn to perform different tasks, many appear to exhibit behavior related to the syntactic and semantic structure of the sentences.
```bgraph-paragraph
{"id":"2cca9777-3338-5121-9565-7d8578e87636","node_type":"Paragraph","location":{"semantic":{"path":"2.8.5","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","4 Why Self-Attention"]},"physical":{"page":7,"bounding_box":{"x":107.6,"y":74.5,"width":397.6,"height":205.90002}}},"text_order":68,"token_count":422,"internal_refs":[{"text":"","source_page":7,"source_bbox":{"x":110.3,"y":96.0,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}},{"text":"","source_page":7,"source_bbox":{"x":185.1,"y":96.0,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.sennrich2015neural","page":11,"point":{"x":108.0,"y":311.0}}},{"text":"","source_page":7,"source_bbox":{"x":315.2,"y":167.0,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.NalBytenet2017","page":10,"point":{"x":108.0,"y":504.0}}},{"text":"","source_page":7,"source_bbox":{"x":350.3,"y":188.8,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.xception2016","page":10,"point":{"x":112.0,"y":113.0}}}],"style":null}
```

## 5 Training
```bgraph-section
{"id":"20ea6355-f05c-57b3-aba3-97b470c501cc","node_type":"Section","location":{"semantic":{"path":"2.9","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training"]},"physical":{"page":7,"bounding_box":{"x":108.0,"y":300.7,"width":62.2,"height":10.6}}},"text_order":69,"token_count":2,"style":null}
```

This section describes the training regime for our models.
```bgraph-paragraph
{"id":"e29ddd2d-5a29-5ea4-a6e7-da2feb8c92cc","node_type":"Paragraph","location":{"semantic":{"path":"2.9.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training"]},"physical":{"page":7,"bounding_box":{"x":107.7,"y":326.9,"width":229.8,"height":8.7}}},"text_order":70,"token_count":14,"style":null}
```

### 5.1 Training Data and Batching
```bgraph-section
{"id":"7a423214-3fa4-5009-9560-5a23f8406a03","node_type":"Section","location":{"semantic":{"path":"2.9.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.1 Training Data and Batching"]},"physical":{"page":7,"bounding_box":{"x":108.0,"y":353.5,"width":141.5,"height":8.700012}}},"text_order":71,"token_count":7,"style":null}
```

We trained on the standard WMT 2014 English-German dataset consisting of about 4.5 million sentence pairs. Sentences were encoded using byte-pair encoding [ 3 ], which has a shared source- target vocabulary of about 37000 tokens. For English-French, we used the significantly larger WMT 2014 English-French dataset consisting of 36M sentences and split tokens into a 32000 word-piece vocabulary [ 38 ]. Sentence pairs were batched together by approximate sequence length. Each training batch contained a set of sentence pairs containing approximately 25000 source tokens and 25000 target tokens.
```bgraph-paragraph
{"id":"0b60b551-dbfe-5fb7-90c8-6759c8618c46","node_type":"Paragraph","location":{"semantic":{"path":"2.9.2.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.1 Training Data and Batching"]},"physical":{"page":7,"bounding_box":{"x":107.5,"y":373.8,"width":398.1,"height":74.900024}}},"text_order":72,"token_count":145,"internal_refs":[{"text":"","source_page":7,"source_bbox":{"x":380.7,"y":384.4,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.DBLP:journals/corr/BritzGLL17","page":9,"point":{"x":112.0,"y":670.0}}},{"text":"","source_page":7,"source_bbox":{"x":155.2,"y":417.2,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}}],"style":null}
```

### 5.2 Hardware and Schedule
```bgraph-section
{"id":"ae62ad93-b93a-56db-a7ca-ea38763e245d","node_type":"Section","location":{"semantic":{"path":"2.9.3","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.2 Hardware and Schedule"]},"physical":{"page":7,"bounding_box":{"x":108.0,"y":466.7,"width":125.0,"height":8.700012}}},"text_order":73,"token_count":6,"style":null}
```

We trained our models on one machine with 8 NVIDIA P100 GPUs. For our base models using the hyperparameters described throughout the paper, each training step took about 0.4 seconds. We trained the base models for a total of 100,000 steps or 12 hours. For our big models,(described on the bottom line of table 3 ), step time was 1.0 seconds. The big models were trained for 300,000 steps (3.5 days).
```bgraph-paragraph
{"id":"e2255733-fed3-5818-be80-7d84a4dda567","node_type":"Paragraph","location":{"semantic":{"path":"2.9.3.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.2 Hardware and Schedule"]},"physical":{"page":7,"bounding_box":{"x":107.5,"y":487.0,"width":396.5,"height":53.0}}},"text_order":74,"token_count":96,"internal_refs":[{"text":"","source_page":7,"source_bbox":{"x":189.4,"y":519.4,"width":7.1,"height":10.9},"target":{"kind":"named","name":"table.3","page":8,"point":{"x":142.0,"y":68.0}}}],"style":null}
```

### 5.3 Optimizer
```bgraph-section
{"id":"76710070-8a7a-5850-a995-79e180e8af87","node_type":"Section","location":{"semantic":{"path":"2.9.4","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.3 Optimizer"]},"physical":{"page":7,"bounding_box":{"x":108.0,"y":558.0,"width":66.09999,"height":8.700012}}},"text_order":75,"token_count":3,"style":null}
```

We used the Adam optimizer [ 20 ] with β = 0 . 9 , β = 0 . 98 and ε = 10 − 9 . We varied the learning 1 2 rate over the course of training, according to the formula:
```bgraph-paragraph
{"id":"366d68e4-c526-58b3-b1a7-e291c3be0ee0","node_type":"Paragraph","location":{"semantic":{"path":"2.9.4.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.3 Optimizer"]},"physical":{"page":7,"bounding_box":{"x":107.5,"y":577.2,"width":396.5,"height":21.400024}}},"text_order":76,"token_count":45,"internal_refs":[{"text":"","source_page":7,"source_bbox":{"x":230.0,"y":578.0,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.kingma2014adam","page":10,"point":{"x":108.0,"y":576.0}}}],"style":null}
```

− − − lrate = d 0 . 5 · min( step _ num 0 . 5 , step _ num · warmup _ steps 1 . 5 ) (3) model
```bgraph-paragraph
{"id":"9c590774-3cec-50a8-9c73-f622d704df9d","node_type":"Paragraph","location":{"semantic":{"path":"2.9.4.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.3 Optimizer"]},"physical":{"page":7,"bounding_box":{"x":162.9,"y":617.8,"width":341.80002,"height":13.400024}}},"text_order":77,"token_count":30,"style":null}
```

This corresponds to increasing the learning rate linearly for the first warmup _ steps training steps, and decreasing it thereafter proportionally to the inverse square root of the step number. We used warmup _ steps = 4000 .
```bgraph-paragraph
{"id":"d40c24c2-a20b-5196-91b5-8edab2c5bee2","node_type":"Paragraph","location":{"semantic":{"path":"2.9.4.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.3 Optimizer"]},"physical":{"page":7,"bounding_box":{"x":107.7,"y":643.0,"width":397.59998,"height":31.299988}}},"text_order":78,"token_count":52,"style":null}
```

### 5.4 Regularization
```bgraph-section
{"id":"c321d525-8c8c-53f0-b455-93face69e625","node_type":"Section","location":{"semantic":{"path":"2.9.5","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.4 Regularization"]},"physical":{"page":7,"bounding_box":{"x":108.0,"y":692.3,"width":85.5,"height":8.7}}},"text_order":79,"token_count":4,"style":null}
```

We employ three types of regularization during training:
```bgraph-paragraph
{"id":"d91588dc-6566-5337-9f9d-b70718ed9503","node_type":"Paragraph","location":{"semantic":{"path":"2.9.5.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.4 Regularization"]},"physical":{"page":7,"bounding_box":{"x":107.5,"y":713.3,"width":224.5,"height":8.7}}},"text_order":80,"token_count":14,"style":null}
```

7
```bgraph-paragraph
{"id":"a887c2c5-0e96-589f-b458-6f6712f5f547","node_type":"Paragraph","location":{"semantic":{"path":"2.9.5.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.4 Regularization"]},"physical":{"page":7,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":81,"token_count":1,"style":null}
```

Table 2: The Transformer achieves better BLEU scores than previous state-of-the-art models on the English-to-German and English-to-French newstest2014 tests at a fraction of the training cost. BLEU Training Cost (FLOPs) Model EN-DE EN-FR EN-DE EN-FR ByteNet [ 18 ] 23.75 Deep-Att + PosUnk [ 39 ] 39.2 1 . 0 · 10 20 GNMT + RL [ 38 ] 24.6 39.92 2 . 3 · 10 19 1 . 4 · 10 20 ConvS2S [ 9 ] 25.16 40.46 9 . 6 · 10 18 1 . 5 · 10 20 MoE [ 32 ] 26.03 40.56 2 . 0 · 10 19 1 . 2 · 10 20 Deep-Att + PosUnk Ensemble [ 39 ] 40.4 8 . 0 · 10 20 GNMT + RL Ensemble [ 38 ] 26.30 41.16 1 . 8 · 10 20 1 . 1 · 10 21 ConvS2S Ensemble [ 9 ] 26.36 41.29 7 . 7 · 10 19 1 . 2 · 10 21 Transformer (base model) 27.3 38.1 3 . 3 · 10 18 Transformer (big) 28.4 41.8 2 . 3 · 10 19
```bgraph-paragraph
{"id":"1fc9716e-a8cb-5494-ac9b-e92e3cc9c296","node_type":"Paragraph","location":{"semantic":{"path":"2.9.5.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.4 Regularization"]},"physical":{"page":8,"bounding_box":{"x":107.7,"y":72.0,"width":396.3,"height":168.09999}}},"text_order":82,"token_count":212,"internal_refs":[{"text":"","source_page":8,"source_bbox":{"x":174.7,"y":124.9,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.NalBytenet2017","page":10,"point":{"x":108.0,"y":504.0}}},{"text":"","source_page":8,"source_bbox":{"x":220.7,"y":136.3,"width":12.0,"height":9.0},"target":{"kind":"named","name":"cite.DBLP:journals/corr/ZhouCWLX16","page":11,"point":{"x":108.0,"y":651.0}}},{"text":"","source_page":8,"source_bbox":{"x":194.1,"y":147.7,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}},{"text":"","source_page":8,"source_bbox":{"x":178.7,"y":159.1,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}},{"text":"","source_page":8,"source_bbox":{"x":161.4,"y":170.5,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.shazeer2017outrageously","page":11,"point":{"x":108.0,"y":345.0}}},{"text":"","source_page":8,"source_bbox":{"x":262.5,"y":183.1,"width":12.0,"height":9.0},"target":{"kind":"named","name":"cite.DBLP:journals/corr/ZhouCWLX16","page":11,"point":{"x":108.0,"y":651.0}}},{"text":"","source_page":8,"source_bbox":{"x":235.9,"y":194.5,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}},{"text":"","source_page":8,"source_bbox":{"x":220.5,"y":205.9,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}}],"style":null}
```

Residual Dropout We apply dropout [ 33 ] to the output of each sub-layer, before it is added to the sub-layer input and normalized. In addition, we apply dropout to the sums of the embeddings and the positional encodings in both the encoder and decoder stacks. For the base model, we use a rate of P = 0 . 1 . drop
```bgraph-paragraph
{"id":"6f10c431-8c71-5ada-a578-d9bdaf741029","node_type":"Paragraph","location":{"semantic":{"path":"2.9.5.4","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.4 Regularization"]},"physical":{"page":8,"bounding_box":{"x":108.0,"y":274.0,"width":396.0,"height":42.200012}}},"text_order":83,"token_count":78,"internal_refs":[{"text":"","source_page":8,"source_bbox":{"x":268.5,"y":273.1,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.srivastava2014dropout","page":11,"point":{"x":108.0,"y":390.0}}}],"style":null}
```

Label Smoothing During training, we employed label smoothing of value ε = 0 . 1 [ 36 ]. This ls hurts perplexity, as the model learns to be more unsure, but improves accuracy and BLEU score.
```bgraph-paragraph
{"id":"bfa8ea41-7915-5eb8-8563-367e03bc8e4b","node_type":"Paragraph","location":{"semantic":{"path":"2.9.5.5","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","5 Training","5.4 Regularization"]},"physical":{"page":8,"bounding_box":{"x":108.0,"y":331.3,"width":396.0,"height":20.50003}}},"text_order":84,"token_count":47,"internal_refs":[{"text":"","source_page":8,"source_bbox":{"x":465.2,"y":331.2,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.DBLP:journals/corr/SzegedyVISW15","page":11,"point":{"x":108.0,"y":526.0}}}],"style":null}
```

## 6 Results
```bgraph-section
{"id":"e4ca45a4-69fa-5b45-9f69-75560e09cf74","node_type":"Section","location":{"semantic":{"path":"2.10","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results"]},"physical":{"page":8,"bounding_box":{"x":108.0,"y":372.1,"width":55.1,"height":10.6}}},"text_order":85,"token_count":2,"style":null}
```

### 6.1 Machine Translation
```bgraph-section
{"id":"c6d80fc4-8ff8-504d-b49a-8c081a54d4a9","node_type":"Section","location":{"semantic":{"path":"2.10.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.1 Machine Translation"]},"physical":{"page":8,"bounding_box":{"x":108.0,"y":398.1,"width":111.09999,"height":8.700012}}},"text_order":86,"token_count":5,"style":null}
```

On the WMT 2014 English-to-German translation task, the big transformer model (Transformer (big) in Table 2 ) outperforms the best previously reported models (including ensembles) by more than 2 . 0 BLEU, establishing a new state-of-the-art BLEU score of 28 . 4 . The configuration of this model is listed in the bottom line of Table 3 . Training took 3 . 5 days on 8 P100 GPUs. Even our base model surpasses all previously published models and ensembles, at a fraction of the training cost of any of the competitive models. On the WMT 2014 English-to-French translation task, our big model achieves a BLEU score of 41 . 0 , outperforming all of the previously published single models, at less than 1 / 4 the training cost of the previous state-of-the-art model. The Transformer (big) model trained for English-to-French used dropout rate P = 0 . 1 , instead of 0 . 3 . drop For the base models, we used a single model obtained by averaging the last 5 checkpoints, which were written at 10-minute intervals. For the big models, we averaged the last 20 checkpoints. We used beam search with a beam size of 4 and length penalty α = 0 . 6 [ 38 ]. These hyperparameters were chosen after experimentation on the development set. We set the maximum output length during inference to input length + 50 , but terminate early when possible [ 38 ]. Table 2 summarizes our results and compares our translation quality and training costs to other model architectures from the literature. We estimate the number of floating point operations used to train a model by multiplying the training time, the number of GPUs used, and an estimate of the sustained single-precision floating-point capacity of each GPU 5 .
```bgraph-paragraph
{"id":"8bd8a9f0-3f9b-57af-b22b-8640aa888eb9","node_type":"Paragraph","location":{"semantic":{"path":"2.10.1.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.1 Machine Translation"]},"physical":{"page":8,"bounding_box":{"x":107.6,"y":419.1,"width":397.59998,"height":221.50003}}},"text_order":87,"token_count":425,"internal_refs":[{"text":"","source_page":8,"source_bbox":{"x":141.3,"y":428.9,"width":6.9,"height":10.9},"target":{"kind":"named","name":"table.2","page":7,"point":{"x":142.0,"y":68.0}}},{"text":"","source_page":8,"source_bbox":{"x":240.9,"y":450.8,"width":7.0,"height":10.9},"target":{"kind":"named","name":"table.3","page":8,"point":{"x":142.0,"y":68.0}}},{"text":"","source_page":8,"source_bbox":{"x":388.8,"y":559.9,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}},{"text":"","source_page":8,"source_bbox":{"x":370.3,"y":581.7,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.wu2016google","page":11,"point":{"x":108.0,"y":595.0}}},{"text":"","source_page":8,"source_bbox":{"x":130.5,"y":598.1,"width":6.9,"height":10.9},"target":{"kind":"named","name":"table.2","page":7,"point":{"x":142.0,"y":68.0}}},{"text":"","source_page":8,"source_bbox":{"x":319.5,"y":629.2,"width":6.0,"height":12.5},"target":{"kind":"named","name":"Hfootnote.2","page":7,"point":{"x":124.0,"y":711.0}}}],"style":null}
```

### 6.2 Model Variations
```bgraph-section
{"id":"a5b59539-3c6c-5b46-b19e-9452c3a79359","node_type":"Section","location":{"semantic":{"path":"2.10.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":8,"bounding_box":{"x":108.0,"y":658.4,"width":95.899994,"height":8.700012}}},"text_order":88,"token_count":5,"style":null}
```

To evaluate the importance of different components of the Transformer, we varied our base model in different ways, measuring the change in performance on English-to-German translation on the
```bgraph-paragraph
{"id":"7b31f2cd-911a-5c15-99c2-815a186a5837","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":8,"bounding_box":{"x":107.7,"y":678.7,"width":396.3,"height":20.499939}}},"text_order":89,"token_count":46,"style":null}
```

5 We used values of 2.8, 3.7, 6.0 and 9.5 TFLOPS for K80, K40, M40 and P100, respectively.
```bgraph-paragraph
{"id":"c9e81fb9-5389-5b3e-aed5-3bac50b7b6ce","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":8,"bounding_box":{"x":120.7,"y":712.5,"width":332.8,"height":9.200012}}},"text_order":90,"token_count":23,"style":null}
```

8
```bgraph-paragraph
{"id":"4bd72d12-5291-5481-a4fb-35450da99abe","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":8,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":91,"token_count":1,"style":null}
```

Table 3: Variations on the Transformer architecture. Unlisted values are identical to those of the base model. All metrics are on the English-to-German translation development set, newstest2013. Listed perplexities are per-wordpiece, according to our byte-pair encoding, and should not be compared to per-word perplexities.
```bgraph-paragraph
{"id":"26d17f96-1e86-5920-96d0-cfa40b3348d1","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.4","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":9,"bounding_box":{"x":107.7,"y":72.0,"width":396.3,"height":41.5}}},"text_order":92,"token_count":78,"style":null}
```

train PPL BLEU params N d d h d d P ε model ff k v drop ls × 6 steps (dev) (dev) 10 base 6 512 2048 8 64 64 0.1 0.1 100K 4.92 25.8 65 1 512 512 5.29 24.9 4 128 128 5.00 25.5 (A) 16 32 32 4.91 25.8 32 16 16 5.01 25.4 16 5.16 25.1 58 (B) 32 5.01 25.4 60 2 6.11 23.7 36 4 5.19 25.3 50 8 4.88 25.5 80 (C) 256 32 32 5.75 24.5 28 1024 128 128 4.66 26.0 168 1024 5.12 25.4 53 4096 4.75 26.2 90 0.0 5.77 24.6 0.2 4.95 25.5 (D) 0.0 4.67 25.3 0.2 5.47 25.7 (E) positional embedding instead of sinusoids 4.92 25.7 big 6 1024 4096 16 0.3 300K 4.33 26.4 213
```bgraph-paragraph
{"id":"dac00a73-5100-5356-9a79-17bd12b4ec40","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.5","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":9,"bounding_box":{"x":116.5,"y":132.3,"width":386.3,"height":250.40001}}},"text_order":93,"token_count":133,"style":null}
```

development set, newstest2013. We used beam search as described in the previous section, but no checkpoint averaging. We present these results in Table 3 .
```bgraph-paragraph
{"id":"f7016dc2-1bca-59f0-a2b3-a05c1ab2d00e","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.6","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":9,"bounding_box":{"x":108.0,"y":413.8,"width":396.0,"height":20.400024}}},"text_order":94,"token_count":38,"internal_refs":[{"text":"","source_page":9,"source_bbox":{"x":330.6,"y":424.5,"width":7.0,"height":10.9},"target":{"kind":"named","name":"table.3","page":8,"point":{"x":142.0,"y":68.0}}}],"style":null}
```

In Table 3 rows (A), we vary the number of attention heads and the attention key and value dimensions, keeping the amount of computation constant, as described in Section 3.2.2 . While single-head attention is 0.9 BLEU worse than the best setting, quality also drops off with too many heads.
```bgraph-paragraph
{"id":"b381ca54-ff97-55f2-914c-2b11004ffc95","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.7","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":9,"bounding_box":{"x":108.0,"y":441.9,"width":397.3,"height":30.50003}}},"text_order":95,"token_count":69,"internal_refs":[{"text":"","source_page":9,"source_bbox":{"x":140.7,"y":440.9,"width":6.9,"height":10.9},"target":{"kind":"named","name":"table.3","page":8,"point":{"x":142.0,"y":68.0}}},{"text":"","source_page":9,"source_bbox":{"x":398.8,"y":451.8,"width":22.3,"height":10.9},"target":{"kind":"named","name":"subsubsection.3.2.2","page":3,"point":{"x":108.0,"y":626.0}}}],"style":null}
```

In Table 3 rows (B), we observe that reducing the attention key size d hurts model quality. This k suggests that determining compatibility is not easy and that a more sophisticated compatibility function than dot product may be beneficial. We further observe in rows (C) and (D) that, as expected, bigger models are better, and dropout is very helpful in avoiding over-fitting. In row (E) we replace our sinusoidal positional encoding with learned positional embeddings [ 9 ], and observe nearly identical results to the base model.
```bgraph-paragraph
{"id":"60719750-78b4-5e74-b34f-b22bbb225880","node_type":"Paragraph","location":{"semantic":{"path":"2.10.2.8","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.2 Model Variations"]},"physical":{"page":9,"bounding_box":{"x":108.0,"y":479.4,"width":397.2,"height":63.899994}}},"text_order":96,"token_count":129,"internal_refs":[{"text":"","source_page":9,"source_bbox":{"x":143.3,"y":478.9,"width":7.1,"height":11.1},"target":{"kind":"named","name":"table.3","page":8,"point":{"x":142.0,"y":68.0}}},{"text":"","source_page":9,"source_bbox":{"x":378.0,"y":522.7,"width":7.0,"height":9.0},"target":{"kind":"named","name":"cite.JonasFaceNet2017","page":10,"point":{"x":112.0,"y":205.0}}}],"style":null}
```

### 6.3 English Constituency Parsing
```bgraph-section
{"id":"42de1b09-131f-584e-bab9-6c2abadbb5cc","node_type":"Section","location":{"semantic":{"path":"2.10.3","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.3 English Constituency Parsing"]},"physical":{"page":9,"bounding_box":{"x":108.0,"y":561.3,"width":148.0,"height":8.700012}}},"text_order":97,"token_count":8,"style":null}
```

To evaluate if the Transformer can generalize to other tasks we performed experiments on English constituency parsing. This task presents specific challenges: the output is subject to strong structural constraints and is significantly longer than the input. Furthermore, RNN sequence-to-sequence models have not been able to attain state-of-the-art results in small-data regimes [ 37 ].
```bgraph-paragraph
{"id":"d1918d9c-2090-5fb9-bad9-2f289db707f8","node_type":"Paragraph","location":{"semantic":{"path":"2.10.3.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.3 English Constituency Parsing"]},"physical":{"page":9,"bounding_box":{"x":107.7,"y":581.6,"width":396.3,"height":42.200012}}},"text_order":98,"token_count":96,"internal_refs":[{"text":"","source_page":9,"source_bbox":{"x":431.3,"y":614.0,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.KVparse15","page":11,"point":{"x":108.0,"y":561.0}}}],"style":null}
```

We trained a 4-layer transformer with d = 1024 on the Wall Street Journal (WSJ) portion of the model Penn Treebank [ 25 ], about 40K training sentences. We also trained it in a semi-supervised setting, using the larger high-confidence and BerkleyParser corpora from with approximately 17M sentences [ 37 ]. We used a vocabulary of 16K tokens for the WSJ only setting and a vocabulary of 32K tokens for the semi-supervised setting.
```bgraph-paragraph
{"id":"b8cf8f04-e5b8-596a-9b7d-b11f6af51e81","node_type":"Paragraph","location":{"semantic":{"path":"2.10.3.2","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.3 English Constituency Parsing"]},"physical":{"page":9,"bounding_box":{"x":107.5,"y":631.3,"width":397.80002,"height":52.5}}},"text_order":99,"token_count":104,"internal_refs":[{"text":"","source_page":9,"source_bbox":{"x":173.6,"y":641.3,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.marcus1993building","page":11,"point":{"x":108.0,"y":72.0}}},{"text":"","source_page":9,"source_bbox":{"x":110.3,"y":663.1,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.KVparse15","page":11,"point":{"x":108.0,"y":561.0}}}],"style":null}
```

We performed only a small number of experiments to select the dropout, both attention and residual (section 5.4 ), learning rates and beam size on the Section 22 development set, all other parameters remained unchanged from the English-to-German base translation model. During inference, we
```bgraph-paragraph
{"id":"29469639-edb3-5719-a2ec-737307d2bd39","node_type":"Paragraph","location":{"semantic":{"path":"2.10.3.3","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.3 English Constituency Parsing"]},"physical":{"page":9,"bounding_box":{"x":107.5,"y":691.5,"width":396.5,"height":30.599976}}},"text_order":100,"token_count":70,"internal_refs":[{"text":"","source_page":9,"source_bbox":{"x":141.2,"y":701.3,"width":14.6,"height":10.9},"target":{"kind":"named","name":"subsection.5.4","page":6,"point":{"x":108.0,"y":685.0}}}],"style":null}
```

9
```bgraph-paragraph
{"id":"2f45411e-b5ce-5137-8883-6d90ca65f22a","node_type":"Paragraph","location":{"semantic":{"path":"2.10.3.4","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.3 English Constituency Parsing"]},"physical":{"page":9,"bounding_box":{"x":303.5,"y":743.2,"width":5.0,"height":8.7}}},"text_order":101,"token_count":1,"style":null}
```

Table 4: The Transformer generalizes well to English constituency parsing (Results are on Section 23 of WSJ) Vinyals & Kaiser el al. (2014) [ 37 ] WSJ only, discriminative 88.3 Petrov et al. (2006) [ 29 ] WSJ only, discriminative 90.4 Zhu et al. (2013) [ 40 ] WSJ only, discriminative 90.4 Dyer et al. (2016) [ 8 ] WSJ only, discriminative 91.7 Transformer (4 layers) WSJ only, discriminative 91.3 Zhu et al. (2013) [ 40 ] semi-supervised 91.3 Huang & Harper (2009) [ 14 ] semi-supervised 91.3 McClosky et al. (2006) [ 26 ] semi-supervised 92.1 Vinyals & Kaiser el al. (2014) [ 37 ] semi-supervised 92.1 Transformer (4 layers) semi-supervised 92.7 Luong et al. (2015) [ 23 ] multi-task 93.0 Dyer et al. (2016) [ 8 ] generative 93.3
```bgraph-paragraph
{"id":"800f444c-0f81-5ab9-9222-089048b6dc61","node_type":"Paragraph","location":{"semantic":{"path":"2.10.3.5","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","6.3 English Constituency Parsing"]},"physical":{"page":10,"bounding_box":{"x":107.7,"y":72.0,"width":396.3,"height":163.09999}}},"text_order":102,"token_count":172,"internal_refs":[{"text":"","source_page":10,"source_bbox":{"x":276.3,"y":105.3,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.KVparse15","page":11,"point":{"x":108.0,"y":561.0}}},{"text":"","source_page":10,"source_bbox":{"x":254.8,"y":116.2,"width":12.0,"height":9.0},"target":{"kind":"named","name":"cite.petrov-EtAl:2006:ACL","page":11,"point":{"x":108.0,"y":220.0}}},{"text":"","source_page":10,"source_bbox":{"x":249.9,"y":127.2,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.zhu-EtAl:2013:ACL","page":11,"point":{"x":108.0,"y":686.0}}},{"text":"","source_page":10,"source_bbox":{"x":254.3,"y":138.1,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.dyer-rnng:16","page":10,"point":{"x":112.0,"y":175.0}}},{"text":"","source_page":10,"source_bbox":{"x":249.9,"y":159.9,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.zhu-EtAl:2013:ACL","page":11,"point":{"x":108.0,"y":686.0}}},{"text":"","source_page":10,"source_bbox":{"x":264.1,"y":170.8,"width":12.0,"height":8.7},"target":{"kind":"named","name":"cite.huang-harper:2009:EMNLP","page":10,"point":{"x":108.0,"y":370.0}}},{"text":"","source_page":10,"source_bbox":{"x":262.5,"y":181.7,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.mcclosky-etAl:2006:NAACL","page":11,"point":{"x":108.0,"y":106.0}}},{"text":"","source_page":10,"source_bbox":{"x":276.3,"y":192.6,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.KVparse15","page":11,"point":{"x":108.0,"y":561.0}}},{"text":"","source_page":10,"source_bbox":{"x":254.8,"y":214.4,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.multiseq2seq","page":10,"point":{"x":108.0,"y":668.0}}},{"text":"","source_page":10,"source_bbox":{"x":254.3,"y":225.3,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.dyer-rnng:16","page":10,"point":{"x":112.0,"y":175.0}}}],"style":null}
```

### Parser Training WSJ 23 F1
```bgraph-section
{"id":"08ceac3c-5d0a-5897-bce1-e2594441bc3d","node_type":"Section","location":{"semantic":{"path":"2.10.4","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","Parser Training WSJ 23 F1"]},"physical":{"page":10,"bounding_box":{"x":206.8,"y":95.0,"width":254.2,"height":8.699997}}},"text_order":103,"token_count":5,"style":null}
```

increased the maximum output length to input length + 300 . We used a beam size of 21 and α = 0 . 3 for both WSJ only and the semi-supervised setting. Our results in Table 4 show that despite the lack of task-specific tuning our model performs sur- prisingly well, yielding better results than all previously reported models with the exception of the Recurrent Neural Network Grammar [ 8 ]. In contrast to RNN sequence-to-sequence models [ 37 ], the Transformer outperforms the Berkeley- Parser [ 29 ] even when training only on the WSJ training set of 40K sentences.
```bgraph-paragraph
{"id":"5431e9be-b358-5e9d-9861-c8ed611d3cd4","node_type":"Paragraph","location":{"semantic":{"path":"2.10.4.1","depth":4,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","6 Results","Parser Training WSJ 23 F1"]},"physical":{"page":10,"bounding_box":{"x":108.0,"y":262.8,"width":397.7,"height":85.20001}}},"text_order":104,"token_count":140,"internal_refs":[{"text":"","source_page":10,"source_bbox":{"x":191.8,"y":289.2,"width":7.1,"height":10.9},"target":{"kind":"named","name":"table.4","page":9,"point":{"x":142.0,"y":68.0}}},{"text":"","source_page":10,"source_bbox":{"x":259.6,"y":311.0,"width":7.0,"height":8.8},"target":{"kind":"named","name":"cite.dyer-rnng:16","page":10,"point":{"x":112.0,"y":175.0}}},{"text":"","source_page":10,"source_bbox":{"x":312.6,"y":327.4,"width":12.0,"height":8.8},"target":{"kind":"named","name":"cite.KVparse15","page":11,"point":{"x":108.0,"y":561.0}}},{"text":"","source_page":10,"source_bbox":{"x":137.6,"y":338.3,"width":12.0,"height":9.0},"target":{"kind":"named","name":"cite.petrov-EtAl:2006:ACL","page":11,"point":{"x":108.0,"y":220.0}}}],"style":null}
```

## 7 Conclusion
```bgraph-section
{"id":"0226993a-4852-51d4-a8bf-da38a46b348e","node_type":"Section","location":{"semantic":{"path":"2.11","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","7 Conclusion"]},"physical":{"page":10,"bounding_box":{"x":108.0,"y":365.6,"width":75.1,"height":10.6}}},"text_order":105,"token_count":3,"style":null}
```

In this work, we presented the Transformer, the first sequence transduction model based entirely on attention, replacing the recurrent layers most commonly used in encoder-decoder architectures with multi-headed self-attention. For translation tasks, the Transformer can be trained significantly faster than architectures based on recurrent or convolutional layers. On both WMT 2014 English-to-German and WMT 2014 English-to-French translation tasks, we achieve a new state of the art. In the former task our best model outperforms even all previously reported ensembles. We are excited about the future of attention-based models and plan to apply them to other tasks. We plan to extend the Transformer to problems involving input and output modalities other than text and to investigate local, restricted attention mechanisms to efficiently handle large inputs and outputs such as images, audio and video. Making generation less sequential is another research goals of ours. The code we used to train and evaluate our models is available at https://github.com/ tensorflow/tensor2tensor .
```bgraph-paragraph
{"id":"4e06f97f-7ce9-5315-9260-bff926828da0","node_type":"Paragraph","location":{"semantic":{"path":"2.11.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","7 Conclusion"]},"physical":{"page":10,"bounding_box":{"x":107.5,"y":389.9,"width":398.1,"height":156.1}}},"text_order":106,"token_count":264,"external_refs":[{"text":"","source_page":10,"source_bbox":{"x":404.7,"y":525.2,"width":100.3,"height":11.1},"target":{"kind":"uri","url":"https://github.com/tensorflow/tensor2tensor"}},{"text":"","source_page":10,"source_bbox":{"x":107.0,"y":536.1,"width":127.5,"height":9.7},"target":{"kind":"uri","url":"https://github.com/tensorflow/tensor2tensor"}}],"style":null}
```

Acknowledgements We are grateful to Nal Kalchbrenner and Stephan Gouws for their fruitful comments, corrections and inspiration.
```bgraph-paragraph
{"id":"e9527adc-4227-560b-8a1c-6b5f83e4dc1c","node_type":"Paragraph","location":{"semantic":{"path":"2.11.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","7 Conclusion"]},"physical":{"page":10,"bounding_box":{"x":108.0,"y":559.1,"width":396.0,"height":20.500061}}},"text_order":107,"token_count":31,"style":null}
```

## References
```bgraph-section
{"id":"7e64fe23-f2a1-57d6-a491-54ec33d8868c","node_type":"Section","location":{"semantic":{"path":"2.12","depth":2,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":10,"bounding_box":{"x":108.0,"y":597.2,"width":55.5,"height":10.6}}},"text_order":108,"token_count":2,"style":null}
```

[1] Jimmy Lei Ba, Jamie Ryan Kiros, and Geoffrey E Hinton. Layer normalization. arXiv preprint arXiv:1607.06450 , 2016. [2] Dzmitry Bahdanau, Kyunghyun Cho, and Yoshua Bengio. Neural machine translation by jointly learning to align and translate. CoRR , abs/1409.0473, 2014.
```bgraph-paragraph
{"id":"9f94141c-8ae4-5236-af78-dff5bf134e7e","node_type":"Paragraph","location":{"semantic":{"path":"2.12.1","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":10,"bounding_box":{"x":113.0,"y":616.0,"width":391.4,"height":48.400024}}},"text_order":109,"token_count":64,"external_refs":[{"text":"","source_page":10,"source_bbox":{"x":128.6,"y":625.9,"width":74.8,"height":10.2},"target":{"kind":"uri","url":"http://arxiv.org/abs/1607.06450"}}],"style":null}
```

[3] Denny Britz, Anna Goldie, Minh-Thang Luong, and Quoc V. Le. Massive exploration of neural machine translation architectures. CoRR , abs/1703.03906, 2017.
```bgraph-paragraph
{"id":"ff0652ab-d825-5b63-a014-733a2db4cd84","node_type":"Paragraph","location":{"semantic":{"path":"2.12.2","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":10,"bounding_box":{"x":113.0,"y":673.6,"width":391.0,"height":19.600037}}},"text_order":110,"token_count":37,"style":null}
```

[4] Jianpeng Cheng, Li Dong, and Mirella Lapata. Long short-term memory-networks for machine reading. arXiv preprint arXiv:1601.06733 , 2016.
```bgraph-paragraph
{"id":"95d42185-4de4-55ce-8c92-7609a3f08cea","node_type":"Paragraph","location":{"semantic":{"path":"2.12.3","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":10,"bounding_box":{"x":113.0,"y":702.4,"width":391.0,"height":19.599976}}},"text_order":111,"token_count":33,"external_refs":[{"text":"","source_page":10,"source_bbox":{"x":223.9,"y":712.2,"width":74.8,"height":10.9},"target":{"kind":"uri","url":"http://arxiv.org/abs/1601.06733"}}],"style":null}
```

10
```bgraph-paragraph
{"id":"0b7a9910-4faa-5b03-abd4-9def6903c7b2","node_type":"Paragraph","location":{"semantic":{"path":"2.12.4","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":10,"bounding_box":{"x":301.0,"y":743.2,"width":10.0,"height":8.7}}},"text_order":112,"token_count":1,"style":null}
```

[5] Kyunghyun Cho, Bart van Merrienboer, Caglar Gulcehre, Fethi Bougares, Holger Schwenk, and Yoshua Bengio. Learning phrase representations using rnn encoder-decoder for statistical machine translation. CoRR , abs/1406.1078, 2014.
```bgraph-paragraph
{"id":"ce10153f-fba2-5a7e-af0f-3c0f749aa347","node_type":"Paragraph","location":{"semantic":{"path":"2.12.5","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":113.0,"y":74.5,"width":392.30002,"height":31.299995}}},"text_order":113,"token_count":56,"style":null}
```

[6] Francois Chollet. Xception: Deep learning with depthwise separable convolutions. arXiv preprint arXiv:1610.02357 , 2016.
```bgraph-paragraph
{"id":"8befaef5-91cc-576d-be64-bb42eb7f927a","node_type":"Paragraph","location":{"semantic":{"path":"2.12.6","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":113.0,"y":116.1,"width":391.0,"height":20.400002}}},"text_order":114,"token_count":29,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":163.4,"y":126.7,"width":74.8,"height":10.8},"target":{"kind":"uri","url":"http://arxiv.org/abs/1610.02357"}}],"style":null}
```

[7] Junyoung Chung, Çaglar Gülçehre, Kyunghyun Cho, and Yoshua Bengio. Empirical evaluation of gated recurrent neural networks on sequence modeling. CoRR , abs/1412.3555, 2014.
```bgraph-paragraph
{"id":"85803598-9be4-5fab-940c-468dc46267d1","node_type":"Paragraph","location":{"semantic":{"path":"2.12.7","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":113.0,"y":147.6,"width":391.0,"height":19.59999}}},"text_order":115,"token_count":43,"style":null}
```

[8] Chris Dyer, Adhiguna Kuncoro, Miguel Ballesteros, and Noah A. Smith. Recurrent neural network grammars. In Proc. of NAACL , 2016.
```bgraph-paragraph
{"id":"cbf2c506-fc9a-5424-b424-ab858159df7e","node_type":"Paragraph","location":{"semantic":{"path":"2.12.8","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":113.0,"y":177.5,"width":391.0,"height":20.399994}}},"text_order":116,"token_count":31,"style":null}
```

[9] Jonas Gehring, Michael Auli, David Grangier, Denis Yarats, and Yann N. Dauphin. Convolu- tional sequence to sequence learning. arXiv preprint arXiv:1705.03122 v2 , 2017.
```bgraph-paragraph
{"id":"0a3fb9de-c4df-515b-a16e-00c930aa1fb1","node_type":"Paragraph","location":{"semantic":{"path":"2.12.9","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":113.0,"y":208.3,"width":392.7,"height":20.299988}}},"text_order":117,"token_count":41,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":340.1,"y":218.9,"width":74.8,"height":10.9},"target":{"kind":"uri","url":"http://arxiv.org/abs/1705.03122"}}],"style":null}
```

[10] Alex Graves. Generating sequences with recurrent neural networks. arXiv preprint arXiv:1308.0850 , 2013.
```bgraph-paragraph
{"id":"b5d69a1e-d021-5e9a-bac9-865d880e0a57","node_type":"Paragraph","location":{"semantic":{"path":"2.12.10","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":239.0,"width":396.0,"height":20.300018}}},"text_order":118,"token_count":24,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":128.6,"y":249.6,"width":69.8,"height":10.2},"target":{"kind":"uri","url":"http://arxiv.org/abs/1308.0850"}}],"style":null}
```

[11] Kaiming He, Xiangyu Zhang, Shaoqing Ren, and Jian Sun. Deep residual learning for im- age recognition. In Proceedings of the IEEE Conference on Computer Vision and Pattern Recognition , pages 770–778, 2016.
```bgraph-paragraph
{"id":"fdfe9ff7-c803-5d64-806f-5fe0d95a73c2","node_type":"Paragraph","location":{"semantic":{"path":"2.12.11","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":269.7,"width":397.7,"height":31.200012}}},"text_order":119,"token_count":50,"style":null}
```

[12] Sepp Hochreiter, Yoshua Bengio, Paolo Frasconi, and Jürgen Schmidhuber. Gradient flow in recurrent nets: the difficulty of learning long-term dependencies, 2001.
```bgraph-paragraph
{"id":"908b9462-1aea-515e-ac34-7c92b71cbc3c","node_type":"Paragraph","location":{"semantic":{"path":"2.12.12","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":311.3,"width":396.0,"height":20.300018}}},"text_order":120,"token_count":41,"style":null}
```

[13] Sepp Hochreiter and Jürgen Schmidhuber. Long short-term memory. Neural computation , 9(8):1735–1780, 1997.
```bgraph-paragraph
{"id":"22f116ca-91ba-5bf8-94da-cf7fbed257f6","node_type":"Paragraph","location":{"semantic":{"path":"2.12.13","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":342.0,"width":397.2,"height":20.400024}}},"text_order":121,"token_count":27,"style":null}
```

[14] Zhongqiang Huang and Mary Harper. Self-training PCFG grammars with latent annotations across languages. In Proceedings of the 2009 Conference on Empirical Methods in Natural Language Processing , pages 832–841. ACL, August 2009.
```bgraph-paragraph
{"id":"557a0ddc-702d-519c-be1c-4489c666ddbb","node_type":"Paragraph","location":{"semantic":{"path":"2.12.14","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":372.7,"width":396.0,"height":31.299988}}},"text_order":122,"token_count":56,"style":null}
```

[15] Rafal Jozefowicz, Oriol Vinyals, Mike Schuster, Noam Shazeer, and Yonghui Wu. Exploring the limits of language modeling. arXiv preprint arXiv:1602.02410 , 2016.
```bgraph-paragraph
{"id":"0083f830-f4e6-5136-a7ca-1606104ec0f1","node_type":"Paragraph","location":{"semantic":{"path":"2.12.15","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":414.3,"width":396.0,"height":20.400024}}},"text_order":123,"token_count":38,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":320.8,"y":424.9,"width":74.8,"height":10.9},"target":{"kind":"uri","url":"http://arxiv.org/abs/1602.02410"}}],"style":null}
```

[16] Łukasz Kaiser and Samy Bengio. Can active memory replace attention? In Advances in Neural Information Processing Systems, (NIPS) , 2016.
```bgraph-paragraph
{"id":"b025ad57-c3f2-5687-ad25-187e4d87fa7d","node_type":"Paragraph","location":{"semantic":{"path":"2.12.16","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":445.8,"width":396.0,"height":19.600037}}},"text_order":124,"token_count":33,"style":null}
```

[17] Łukasz Kaiser and Ilya Sutskever. Neural GPUs learn algorithms. In International Conference on Learning Representations (ICLR) , 2016.
```bgraph-paragraph
{"id":"a0c81791-8dea-5cd8-b389-01b7c8b0745b","node_type":"Paragraph","location":{"semantic":{"path":"2.12.17","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":476.5,"width":396.0,"height":19.600006}}},"text_order":125,"token_count":33,"style":null}
```

[18] Nal Kalchbrenner, Lasse Espeholt, Karen Simonyan, Aaron van den Oord, Alex Graves, and Ko- ray Kavukcuoglu. Neural machine translation in linear time. arXiv preprint arXiv:1610.10099 v2 , 2017.
```bgraph-paragraph
{"id":"07b04cd6-2112-5f4d-9aa0-326a57ef6be2","node_type":"Paragraph","location":{"semantic":{"path":"2.12.18","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":507.2,"width":397.7,"height":30.5}}},"text_order":126,"token_count":47,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":421.3,"y":517.1,"width":73.3,"height":10.9},"target":{"kind":"uri","url":"http://arxiv.org/abs/1610.10099"}}],"style":null}
```

[19] Yoon Kim, Carl Denton, Luong Hoang, and Alexander M. Rush. Structured attention networks. In International Conference on Learning Representations , 2017.
```bgraph-paragraph
{"id":"8a6f0a10-bad2-5b96-920b-4198c89e7b4a","node_type":"Paragraph","location":{"semantic":{"path":"2.12.19","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":548.8,"width":397.7,"height":19.600037}}},"text_order":127,"token_count":38,"style":null}
```

[20] Diederik Kingma and Jimmy Ba. Adam: A method for stochastic optimization. In ICLR , 2015.
```bgraph-paragraph
{"id":"187d409a-bff3-50f4-9eb0-beac1c8de117","node_type":"Paragraph","location":{"semantic":{"path":"2.12.20","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":579.5,"width":397.7,"height":8.700012}}},"text_order":128,"token_count":22,"style":null}
```

[21] Oleksii Kuchaiev and Boris Ginsburg. Factorization tricks for LSTM networks. arXiv preprint arXiv:1703.10722 , 2017.
```bgraph-paragraph
{"id":"34fa571b-c800-5a64-95f7-476a3b543bbd","node_type":"Paragraph","location":{"semantic":{"path":"2.12.21","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":599.3,"width":396.0,"height":19.600037}}},"text_order":129,"token_count":28,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":128.6,"y":609.2,"width":74.8,"height":10.2},"target":{"kind":"uri","url":"http://arxiv.org/abs/1703.10722"}}],"style":null}
```

[22] Zhouhan Lin, Minwei Feng, Cicero Nogueira dos Santos, Mo Yu, Bing Xiang, Bowen Zhou, and Yoshua Bengio. A structured self-attentive sentence embedding. arXiv preprint arXiv:1703.03130 , 2017.
```bgraph-paragraph
{"id":"08774686-792c-5412-8dec-02b4e9177eb2","node_type":"Paragraph","location":{"semantic":{"path":"2.12.22","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":629.3,"width":396.0,"height":31.300049}}},"text_order":130,"token_count":46,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":128.6,"y":650.8,"width":74.8,"height":10.2},"target":{"kind":"uri","url":"http://arxiv.org/abs/1703.03130"}}],"style":null}
```

[23] Minh-Thang Luong, Quoc V. Le, Ilya Sutskever, Oriol Vinyals, and Lukasz Kaiser. Multi-task sequence to sequence learning. arXiv preprint arXiv:1511.06114 , 2015.
```bgraph-paragraph
{"id":"219bc91f-0912-53dd-9dfe-9e4cbed8c01f","node_type":"Paragraph","location":{"semantic":{"path":"2.12.23","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":671.7,"width":396.3,"height":19.599976}}},"text_order":131,"token_count":38,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":315.0,"y":681.5,"width":74.8,"height":10.9},"target":{"kind":"uri","url":"http://arxiv.org/abs/1511.06114"}}],"style":null}
```

[24] Minh-Thang Luong, Hieu Pham, and Christopher D Manning. Effective approaches to attention- based neural machine translation. arXiv preprint arXiv:1508.04025 , 2015.
```bgraph-paragraph
{"id":"bf9a9a81-4d96-55ac-8421-c439b71999ec","node_type":"Paragraph","location":{"semantic":{"path":"2.12.24","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":108.0,"y":702.4,"width":397.7,"height":19.599976}}},"text_order":132,"token_count":39,"external_refs":[{"text":"","source_page":11,"source_bbox":{"x":324.9,"y":712.2,"width":74.8,"height":10.8},"target":{"kind":"uri","url":"http://arxiv.org/abs/1508.04025"}}],"style":null}
```

11
```bgraph-paragraph
{"id":"169a45d0-fc17-57ae-99e5-1c966281984b","node_type":"Paragraph","location":{"semantic":{"path":"2.12.25","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":11,"bounding_box":{"x":301.0,"y":743.2,"width":10.0,"height":8.7}}},"text_order":133,"token_count":1,"style":null}
```

[25] Mitchell P Marcus, Mary Ann Marcinkiewicz, and Beatrice Santorini. Building a large annotated corpus of english: The penn treebank. Computational linguistics , 19(2):313–330, 1993.
```bgraph-paragraph
{"id":"5dea53e7-2149-52d4-aee2-57798bae40b4","node_type":"Paragraph","location":{"semantic":{"path":"2.12.26","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":75.3,"width":396.0,"height":19.59999}}},"text_order":134,"token_count":45,"style":null}
```

[26] David McClosky, Eugene Charniak, and Mark Johnson. Effective self-training for parsing. In Proceedings of the Human Language Technology Conference of the NAACL, Main Conference , pages 152–159. ACL, June 2006.
```bgraph-paragraph
{"id":"6277b494-a65c-51fb-a044-d89030c40c5d","node_type":"Paragraph","location":{"semantic":{"path":"2.12.27","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":109.6,"width":397.3,"height":30.599998}}},"text_order":135,"token_count":53,"style":null}
```

[27] Ankur Parikh, Oscar Täckström, Dipanjan Das, and Jakob Uszkoreit. A decomposable attention model. In Empirical Methods in Natural Language Processing , 2016.
```bgraph-paragraph
{"id":"f74eb752-4581-55b1-a740-3d7fc0fde9db","node_type":"Paragraph","location":{"semantic":{"path":"2.12.28","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":154.9,"width":396.0,"height":19.699997}}},"text_order":136,"token_count":39,"style":null}
```

[28] Romain Paulus, Caiming Xiong, and Richard Socher. A deep reinforced model for abstractive summarization. arXiv preprint arXiv:1705.04304 , 2017.
```bgraph-paragraph
{"id":"ee3b94d5-b132-58fd-b333-33f12b4c6339","node_type":"Paragraph","location":{"semantic":{"path":"2.12.29","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":189.3,"width":396.0,"height":19.59999}}},"text_order":137,"token_count":34,"external_refs":[{"text":"","source_page":12,"source_bbox":{"x":253.3,"y":199.2,"width":74.8,"height":10.8},"target":{"kind":"uri","url":"http://arxiv.org/abs/1705.04304"}}],"style":null}
```

[29] Slav Petrov, Leon Barrett, Romain Thibaux, and Dan Klein. Learning accurate, compact, and interpretable tree annotation. In Proceedings of the 21st International Conference on Computational Linguistics and 44th Annual Meeting of the ACL , pages 433–440. ACL, July 2006.
```bgraph-paragraph
{"id":"926afeea-9cca-5e78-a41b-c257efc4876f","node_type":"Paragraph","location":{"semantic":{"path":"2.12.30","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":223.0,"width":397.30002,"height":42.200012}}},"text_order":138,"token_count":66,"style":null}
```

[30] Ofir Press and Lior Wolf. Using the output embedding to improve language models. arXiv preprint arXiv:1608.05859 , 2016.
```bgraph-paragraph
{"id":"eef5d9b1-72e8-5437-a43f-592d000884dd","node_type":"Paragraph","location":{"semantic":{"path":"2.12.31","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":279.2,"width":396.0,"height":20.299988}}},"text_order":139,"token_count":29,"external_refs":[{"text":"","source_page":12,"source_bbox":{"x":163.4,"y":289.8,"width":74.8,"height":10.8},"target":{"kind":"uri","url":"http://arxiv.org/abs/1608.05859"}}],"style":null}
```

[31] Rico Sennrich, Barry Haddow, and Alexandra Birch. Neural machine translation of rare words with subword units. arXiv preprint arXiv:1508.07909 , 2015.
```bgraph-paragraph
{"id":"0255b3b3-da26-5a52-b7bd-fd7a56e07b5e","node_type":"Paragraph","location":{"semantic":{"path":"2.12.32","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":314.3,"width":396.0,"height":19.600037}}},"text_order":140,"token_count":35,"external_refs":[{"text":"","source_page":12,"source_bbox":{"x":270.0,"y":324.2,"width":74.8,"height":10.8},"target":{"kind":"uri","url":"http://arxiv.org/abs/1508.07909"}}],"style":null}
```

[32] Noam Shazeer, Azalia Mirhoseini, Krzysztof Maziarz, Andy Davis, Quoc Le, Geoffrey Hinton, and Jeff Dean. Outrageously large neural networks: The sparsely-gated mixture-of-experts layer. arXiv preprint arXiv:1701.06538 , 2017.
```bgraph-paragraph
{"id":"593e279d-23b5-5e04-9efe-18333ba4468e","node_type":"Paragraph","location":{"semantic":{"path":"2.12.33","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":348.7,"width":397.2,"height":30.5}}},"text_order":141,"token_count":54,"external_refs":[{"text":"","source_page":12,"source_bbox":{"x":213.4,"y":369.5,"width":74.8,"height":10.9},"target":{"kind":"uri","url":"http://arxiv.org/abs/1701.06538"}}],"style":null}
```

[33] Nitish Srivastava, Geoffrey E Hinton, Alex Krizhevsky, Ilya Sutskever, and Ruslan Salakhutdi- nov. Dropout: a simple way to prevent neural networks from overfitting. Journal of Machine Learning Research , 15(1):1929–1958, 2014.
```bgraph-paragraph
{"id":"169e8245-7102-59e9-8d8b-650a049a6dcf","node_type":"Paragraph","location":{"semantic":{"path":"2.12.34","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":394.0,"width":397.7,"height":30.5}}},"text_order":142,"token_count":55,"style":null}
```

[34] Sainbayar Sukhbaatar, Arthur Szlam, Jason Weston, and Rob Fergus. End-to-end memory networks. In C. Cortes, N. D. Lawrence, D. D. Lee, M. Sugiyama, and R. Garnett, editors, Advances in Neural Information Processing Systems 28 , pages 2440–2448. Curran Associates, Inc., 2015.
```bgraph-paragraph
{"id":"b06e82fe-2338-5643-95eb-3bf9df594940","node_type":"Paragraph","location":{"semantic":{"path":"2.12.35","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":438.6,"width":397.30002,"height":42.200012}}},"text_order":143,"token_count":67,"style":null}
```

[35] Ilya Sutskever, Oriol Vinyals, and Quoc VV Le. Sequence to sequence learning with neural networks. In Advances in Neural Information Processing Systems , pages 3104–3112, 2014.
```bgraph-paragraph
{"id":"699ea568-2ca4-5934-9541-a339768312a8","node_type":"Paragraph","location":{"semantic":{"path":"2.12.36","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":494.8,"width":396.0,"height":20.299988}}},"text_order":144,"token_count":44,"style":null}
```

[36] Christian Szegedy, Vincent Vanhoucke, Sergey Ioffe, Jonathon Shlens, and Zbigniew Wojna. Rethinking the inception architecture for computer vision. CoRR , abs/1512.00567, 2015.
```bgraph-paragraph
{"id":"a7abd9c2-9425-5952-8d4f-8887a41297be","node_type":"Paragraph","location":{"semantic":{"path":"2.12.37","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":529.2,"width":397.80002,"height":20.299988}}},"text_order":145,"token_count":43,"style":null}
```

[37] Vinyals & Kaiser, Koo, Petrov, Sutskever, and Hinton. Grammar as a foreign language. In Advances in Neural Information Processing Systems , 2015.
```bgraph-paragraph
{"id":"ec5a2870-e812-5923-98fd-d5972f6df10f","node_type":"Paragraph","location":{"semantic":{"path":"2.12.38","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":563.6,"width":396.0,"height":20.300049}}},"text_order":146,"token_count":35,"style":null}
```

[38] Yonghui Wu, Mike Schuster, Zhifeng Chen, Quoc V Le, Mohammad Norouzi, Wolfgang Macherey, Maxim Krikun, Yuan Cao, Qin Gao, Klaus Macherey, et al. Google’s neural machine translation system: Bridging the gap between human and machine translation. arXiv preprint arXiv:1609.08144 , 2016.
```bgraph-paragraph
{"id":"6469154b-8555-5960-bbf4-22629940598d","node_type":"Paragraph","location":{"semantic":{"path":"2.12.39","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":598.0,"width":396.0,"height":42.100037}}},"text_order":147,"token_count":68,"external_refs":[{"text":"","source_page":12,"source_bbox":{"x":128.6,"y":630.4,"width":74.8,"height":10.2},"target":{"kind":"uri","url":"http://arxiv.org/abs/1609.08144"}}],"style":null}
```

[39] Jie Zhou, Ying Cao, Xuguang Wang, Peng Li, and Wei Xu. Deep recurrent models with fast-forward connections for neural machine translation. CoRR , abs/1606.04199, 2016.
```bgraph-paragraph
{"id":"8cc4be10-89bb-546a-b1af-f68c32a5ec74","node_type":"Paragraph","location":{"semantic":{"path":"2.12.40","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":654.2,"width":396.0,"height":20.299988}}},"text_order":148,"token_count":41,"style":null}
```

[40] Muhua Zhu, Yue Zhang, Wenliang Chen, Min Zhang, and Jingbo Zhu. Fast and accurate shift-reduce constituent parsing. In Proceedings of the 51st Annual Meeting of the ACL (Volume 1: Long Papers) , pages 434–443. ACL, August 2013.
```bgraph-paragraph
{"id":"caf54f1e-edd8-5235-b2f0-93549e3e0a83","node_type":"Paragraph","location":{"semantic":{"path":"2.12.41","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":108.0,"y":688.6,"width":396.0,"height":31.200012}}},"text_order":149,"token_count":56,"style":null}
```

12
```bgraph-paragraph
{"id":"54a52c91-21c0-598b-bca9-5637a9b1e1ee","node_type":"Paragraph","location":{"semantic":{"path":"2.12.42","depth":3,"breadcrumbs":["Attention Is All You Need","Attention Is All You Need","References"]},"physical":{"page":12,"bounding_box":{"x":301.0,"y":743.2,"width":10.0,"height":8.7}}},"text_order":150,"token_count":1,"style":null}
```

# **Input-Input Layer5**
```bgraph-section
{"id":"8ded40a0-d20e-5e2e-a472-c02b774fddaf","node_type":"Section","location":{"semantic":{"path":"3","depth":1,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":13,"bounding_box":{"x":108.0,"y":60.5,"width":142.5,"height":22.4}}},"text_order":151,"token_count":4,"style":null}
```

## Attention Visualizations
```bgraph-section
{"id":"1d260aab-39cd-5faf-9702-297343932824","node_type":"Section","location":{"semantic":{"path":"3.1","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":108.0,"y":73.6,"width":122.8,"height":10.6}}},"text_order":152,"token_count":6,"style":null}
```

s t n n n e io t y a m a s t it c n d g r s l > r ri r e n t g e u S > > > > > > i it o e e s e 9 s n e c d d d d d d r t j e v v s w s c 0 k i i c r i O a a a a a a is i a a m w n a e g t o o ff E p p p p p p f o a a e 0 r r i t h p h i h e o I is in t s t a m o A g h p n la s 2 m t r o v p m d . < < < < < < <
```bgraph-paragraph
{"id":"caf0b43b-fe69-5a6a-ba09-f897f489f436","node_type":"Paragraph","location":{"semantic":{"path":"3.1.1","depth":3,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":127.4,"y":111.7,"width":373.5,"height":51.0}}},"text_order":153,"token_count":73,"style":null}
```

t t t f t . I is in is i a ty n ts e d w s e 9 g e n r g s e l > > > > > > > r a i o o s r h i a v e e w c 0 n h o n u S d d d d d d t p th r c n a s a n 0 i t ti ti e o ic a a a a a a i n i k s jo r e h s l s 2 a o c m ff O p p p p p p a a e m a tr v o i E < < < < < < r p m d m m rn is p < A e g v e r o g
```bgraph-paragraph
{"id":"7b230b2c-f43a-5f99-9d2b-4276a2c99784","node_type":"Paragraph","location":{"semantic":{"path":"3.1.2","depth":3,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":127.4,"y":235.8,"width":373.5,"height":52.800003}}},"text_order":154,"token_count":72,"style":null}
```

Figure 3: An example of the attention mechanism following long-distance dependencies in the
```bgraph-paragraph
{"id":"24969be1-a4e8-599c-8fd7-0664eca4f40d","node_type":"Paragraph","location":{"semantic":{"path":"3.1.3","depth":3,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":108.0,"y":312.5,"width":396.0,"height":9.6}}},"text_order":155,"token_count":22,"style":null}
```

encoder self-attention in layer 5 of 6. Many of the attention heads attend to a distant dependency of
```bgraph-paragraph
{"id":"7948a380-54e6-5fe6-bbac-bb9ebc361edd","node_type":"Paragraph","location":{"semantic":{"path":"3.1.4","depth":3,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":108.0,"y":324.1,"width":396.0,"height":8.7}}},"text_order":156,"token_count":25,"style":null}
```

the verb ‘making’, completing the phrase ‘making...more difficult’. Attentions here shown only for
```bgraph-paragraph
{"id":"817d04d9-8350-5cc8-9721-70842b0f2de4","node_type":"Paragraph","location":{"semantic":{"path":"3.1.5","depth":3,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":108.0,"y":334.3,"width":396.2,"height":9.6}}},"text_order":157,"token_count":26,"style":null}
```

the word ‘making’. Different colors represent different heads. Best viewed in color.
```bgraph-paragraph
{"id":"b72287cb-64d7-5c39-9355-51633724fd0c","node_type":"Paragraph","location":{"semantic":{"path":"3.1.6","depth":3,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":108.0,"y":346.0,"width":332.9,"height":8.7}}},"text_order":158,"token_count":22,"style":null}
```

13
```bgraph-paragraph
{"id":"4b8ae775-8b73-5092-9d33-9816be1270b1","node_type":"Paragraph","location":{"semantic":{"path":"3.1.7","depth":3,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**","Attention Visualizations"]},"physical":{"page":13,"bounding_box":{"x":301.0,"y":743.2,"width":10.0,"height":8.7}}},"text_order":159,"token_count":1,"style":null}
```

# **Input-Input Layer5**
```bgraph-section
{"id":"f3f6a5d9-f20b-5aff-9aaf-c0b2264a8af7","node_type":"Section","location":{"semantic":{"path":"4","depth":1,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":108.0,"y":98.7,"width":170.7,"height":22.4}}},"text_order":160,"token_count":4,"style":null}
```

n o i t t a g n > d r c c l in o S > i t i e fe l u s d e w l v r t p o t s a s in O a l i e i y h a i e e e u s p h e s h e r p E p t u h s n T L w n b p , b i a s b j - t i w w a m , i m o . < <
```bgraph-paragraph
{"id":"fadb4b1c-15f8-5fcf-b4a3-3166158ffcd1","node_type":"Paragraph","location":{"semantic":{"path":"4.1","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":131.3,"y":168.1,"width":363.40002,"height":49.399994}}},"text_order":161,"token_count":47,"style":null}
```

l r t , t t - t , . e w il e s n d e is is e e g n y n > > e c u t l s a r i Input-Input Layer5 h a b i o b h w n o S d w v e b i u u h a i m i T L f t o j t s n a e r w i O a p n e c h is p E < p li s o m < p p a
```bgraph-paragraph
{"id":"b1b49abf-f8a8-5ccf-9f4c-b1f334bab1b8","node_type":"Paragraph","location":{"semantic":{"path":"4.2","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":108.0,"y":306.4,"width":386.7,"height":52.5}}},"text_order":162,"token_count":48,"style":null}
```

n o i t t a g n > d r c c l in o S > i t i e fe l u s d e w l v r t p o t s a s in O a l i e i y h a i e e e u s p h e s h e r p E p t u h s n T L w n b p , b i a s b j - t i w w a m , i m o . < <
```bgraph-paragraph
{"id":"9804f960-e313-5fcb-a19d-e631e2684b8b","node_type":"Paragraph","location":{"semantic":{"path":"4.3","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":131.6,"y":384.9,"width":367.5,"height":49.80002}}},"text_order":163,"token_count":47,"style":null}
```

l r t , t t - t , . e w il e s n d e is is e e g n y n > > e c u t l s a r i h a b i o b h w n o S d w v e b i u u h a i m i T L f t o j t s n a e r w i O a p n e c h is p E < p li s o m < p p a
```bgraph-paragraph
{"id":"bd7955fe-8737-5cce-9127-9bfa80c3674d","node_type":"Paragraph","location":{"semantic":{"path":"4.4","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":131.6,"y":524.8,"width":367.5,"height":53.0}}},"text_order":164,"token_count":44,"style":null}
```

Figure 4: Two attention heads, also in layer 5 of 6, apparently involved in anaphora resolution. Top:
```bgraph-paragraph
{"id":"57c099cc-ea02-5ba5-a728-82d4ff88b762","node_type":"Paragraph","location":{"semantic":{"path":"4.5","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":108.0,"y":615.1,"width":397.4,"height":8.7}}},"text_order":165,"token_count":25,"style":null}
```

Full attentions for head 5. Bottom: Isolated attentions from just the word ‘its’ for attention heads 5
```bgraph-paragraph
{"id":"6e0aca31-3ef6-59e0-b94c-68e75a075ee8","node_type":"Paragraph","location":{"semantic":{"path":"4.6","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":108.0,"y":625.3,"width":396.0,"height":9.6}}},"text_order":166,"token_count":26,"style":null}
```

and 6. Note that the attentions are very sharp for this word.
```bgraph-paragraph
{"id":"c5fc3b4b-a072-51c7-8bfe-1df15712f793","node_type":"Paragraph","location":{"semantic":{"path":"4.7","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":108.0,"y":636.9,"width":235.3,"height":8.7}}},"text_order":167,"token_count":15,"style":null}
```

14
```bgraph-paragraph
{"id":"fb00027b-9d56-5701-9dda-cd8da2e74bd8","node_type":"Paragraph","location":{"semantic":{"path":"4.8","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":14,"bounding_box":{"x":301.0,"y":743.2,"width":10.0,"height":8.7}}},"text_order":168,"token_count":1,"style":null}
```

# **Input-Input Layer5**
```bgraph-section
{"id":"22a40c6a-30d0-5a6a-9964-42e0443f04d6","node_type":"Section","location":{"semantic":{"path":"5","depth":1,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":107.0,"y":113.0,"width":171.1,"height":22.4}}},"text_order":169,"token_count":4,"style":null}
```

n o i t t a g n > d r c c l in o S > i t i e fe l u s d e w l v r t p o t s a s in O a l i e i y h a i e e e u s p h e s h e r p E p t u h s n T L w n b p , b i a s b j - t i w w a m , i m o . < <
```bgraph-paragraph
{"id":"3742850b-6a12-5770-9969-b1d4079e123c","node_type":"Paragraph","location":{"semantic":{"path":"5.1","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":130.3,"y":182.6,"width":364.40002,"height":49.399994}}},"text_order":170,"token_count":47,"style":null}
```

l r t , t t - t , . e w il e s n d e is is e e g n y n > > e c u t l s a r i Input-Input Layer5 h a b i o b h w n o S d w v e b i u u h a i m i T L f t o j t s n a e r w i O a p n e c h is p E < p li s o m < p p a
```bgraph-paragraph
{"id":"d8924d72-b9f1-51f0-8c1c-f92c2c7420c1","node_type":"Paragraph","location":{"semantic":{"path":"5.2","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":106.8,"y":321.3,"width":387.90002,"height":52.5}}},"text_order":171,"token_count":48,"style":null}
```

n o i t t a g n > d r c c l in o S > i t i e fe l u s d e w l v r t p o t s a s in O a l i e i y h a i e e e u s p h e s h e r p E p t u h s n T L w n b p , b i a s b j - t i w w a m , i m o . < <
```bgraph-paragraph
{"id":"fca3ccf9-94a7-5fd8-89ef-38a725daed89","node_type":"Paragraph","location":{"semantic":{"path":"5.3","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":130.1,"y":400.8,"width":364.6,"height":49.5}}},"text_order":172,"token_count":47,"style":null}
```

l r t , t t - t , . e w il e s n d e is is e e g n y n > > e c u t l s a r i h a b i o b h w n o S d w v e b i u u h a i m i T L f t o j t s n a e r w i O a p n e c h is p E < p li s o m < p p a
```bgraph-paragraph
{"id":"6adb70ea-eafd-569f-9d5e-69a73bda33de","node_type":"Paragraph","location":{"semantic":{"path":"5.4","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":130.1,"y":539.6,"width":364.6,"height":52.600037}}},"text_order":173,"token_count":44,"style":null}
```

Figure 5: Many of the attention heads exhibit behaviour that seems related to the structure of the
```bgraph-paragraph
{"id":"2b17a963-79a4-558f-a4da-8f83292219b0","node_type":"Paragraph","location":{"semantic":{"path":"5.5","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":108.0,"y":602.5,"width":396.0,"height":9.6}}},"text_order":174,"token_count":24,"style":null}
```

sentence. We give two such examples above, from two different heads from the encoder self-attention
```bgraph-paragraph
{"id":"d718250d-6f33-56ca-9b38-cbe5d1670ba3","node_type":"Paragraph","location":{"semantic":{"path":"5.6","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":108.0,"y":614.2,"width":396.0,"height":8.7}}},"text_order":175,"token_count":24,"style":null}
```

at layer 5 of 6. The heads clearly learned to perform different tasks.
```bgraph-paragraph
{"id":"ab1b2b43-ebcb-5abd-bf6a-413caf2cebe7","node_type":"Paragraph","location":{"semantic":{"path":"5.7","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":108.0,"y":625.1,"width":269.3,"height":8.7}}},"text_order":176,"token_count":17,"style":null}
```

15
```bgraph-paragraph
{"id":"b1df1c86-bc27-5b3a-98f6-bf85498ecf88","node_type":"Paragraph","location":{"semantic":{"path":"5.8","depth":2,"breadcrumbs":["Attention Is All You Need","**Input-Input Layer5**"]},"physical":{"page":15,"bounding_box":{"x":301.0,"y":743.2,"width":10.0,"height":8.7}}},"text_order":177,"token_count":1,"style":null}
```
