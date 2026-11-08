<script setup lang="ts">
import DefaultTheme from "vitepress/theme";
import "./style.css";

const { Layout } = DefaultTheme;
</script>

<template>
  <Layout>
    <template #home-hero-before>
      <div class="mio-marquee" aria-hidden="true">
        <span>MIO</span>
        <span class="mio-marquee__dot" />
        <span>TYPESCRIPT RUNTIME</span>
        <span class="mio-marquee__dot" />
        <span>MANUAL / 001</span>
      </div>
    </template>

    <template #home-features-after>
      <section class="mio-manifesto">
        <div class="mio-manifesto__block mio-manifesto__block--red">NO</div>
        <div class="mio-manifesto__text">
          <p class="mio-manifesto__kicker">MANIFESTO</p>
          <h2>不模仿 Node。<br />不假装是浏览器。</h2>
          <p>
            mio 只提供一类东西：Web 标准 API 与一个 TypeScript 运行时。
            没有 <code>node_modules</code>，没有 CommonJS，没有 <code>window</code>。
            少一层兼容，就少一层解释。
          </p>
        </div>
      </section>
    </template>
  </Layout>
</template>
