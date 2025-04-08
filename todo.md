# TODO: Evolving the Search Ranking Algorithm

## Phase 1: Transition to BM25

### Research BM25 Fundamentals
- Study the BM25 formula, including term frequency and document length normalization.
- Review related literature (e.g., the Okapi BM25 paper) and existing implementations for guidance.

### Analyze Current Algorithm
- Document your current TF-IDF (or other) ranking methodology.
- Identify the key differences in scoring compared to BM25.

### Implement BM25 Scoring Function
- Define BM25 parameters (e.g., `k1` and `b`).
- Write a function that calculates BM25 scores for a document given a query.
- Ensure the function processes term frequency and document length correctly.

### Integrate BM25 into the Search Pipeline
- Replace or augment your existing scoring function with BM25.
- Update unit tests to validate BM25 scoring against sample inputs.

### Benchmark and Tune
- Compare BM25 results with your current algorithm on a set of test queries.
- Fine-tune parameters `k1` and `b` for optimal relevance and performance.

---

## Phase 2: Extend BM25 to BM25F (Field-based BM25)

### Research BM25F Concepts
- Understand how BM25F extends BM25 to handle multiple fields (e.g., file name, content, metadata).
- Look into how field weights (boost factors) influence ranking.

### Design Field-Specific Indexing
- Identify and document key fields: file name, file extension, file content, and any additional metadata.
- Decide how to index each field and store them.

### Implement BM25F Scoring
- Create a new function to calculate BM25F scores for a document.
- Incorporate field-level term statistics and boosting factors.
- Write tests to ensure the BM25F function computes scores correctly from multi-field data.

### Integrate BM25F into the Search Pipeline
- Modify your indexing process to store field-level data if not already stored.
- Replace the BM25 function with BM25F for ranking.
- Benchmark the new system and iterate on field weights as needed.