class Multiplexing {

  latencyDurations = [5, 4, 3, 2, 1];

  rootId;
  rootUrl;

  

  constructor(rootId, rootUrl) {
    this.rootId = rootId;
    this.rootUrl = rootUrl;

    this.registerListeners();
    this.render();
  }

  load() {

  }

  render() {

  }

  registerListeners() {

  }

  async startShowcase() {
    for (const duration of this.latencyDurations) {
      fetch(`${this.rootUrl}/slow/`)
    }
    
  }
}