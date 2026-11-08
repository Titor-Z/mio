import { defineConfig } from "vitepress";

// mio 手册站点配置。设计主题在 .vitepress/theme 中重写，不复用默认外观。
export default defineConfig({
  title: "MIO",
  description: "TypeScript 一等公民的运行时 —— 手册",
  lang: "zh-CN",
  srcExclude: ["dist/**", "node_modules/**"],
  outDir: "dist",
  cleanUrls: true,
  themeConfig: {
    siteTitle: "MIO / MANUAL",
    nav: [
      { text: "指南", link: "/guide" },
      { text: "速查", link: "/cheat-sheet" },
      { text: "兼容性", link: "/compatibility" },
    ],
    sidebar: [
      {
        text: "手册",
        items: [
          { text: "上手指南", link: "/guide" },
          { text: "快速参考", link: "/cheat-sheet" },
          { text: "运行时对比", link: "/compatibility" },
        ],
      },
    ],
    outline: { label: "本页", level: [2, 3] },
    docFooter: { prev: "上一页", next: "下一页" },
    darkModeSwitchLabel: "明暗",
    returnToTopLabel: "回到顶部",
    sidebarMenuLabel: "目录",
    lastUpdated: { text: "更新于" },
  },
});
