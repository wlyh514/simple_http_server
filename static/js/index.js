class Showcase {

  rootId;
  rootUrl;
  requestFn;

  constructor(rootId, rootUrl, requestFn) {
    this.rootId = rootId;
    this.rootUrl = rootUrl;
    this.requestFn = requestFn;

    this.load();
    this.registerListeners();
    this.render();
  }

  load() {
    window.addEventListener("DOMContentLoaded", () => {
      console.log(`#${this.rootId}`);
      document
        .querySelector(`#${this.rootId}`)
        .innerHTML = "<b>If you see this paragraph, page js is loaded successfully. </b>";
    })
  }

  render() {

  }

  registerListeners() {

  }

  async startShowcase() {

  }
}