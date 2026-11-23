import { Tabs as TabsPrimitive } from "@base-ui/react/tabs"
import { cn } from "cn"

// Restyled to the prototype's tab row: underlined tabs on the panel colour.

function Tabs({ className, ...props }: TabsPrimitive.Root.Props) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn("flex flex-col", className)}
      {...props}
    />
  )
}

function TabsList({ className, ...props }: TabsPrimitive.List.Props) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn(
        "flex flex-none items-end gap-1 border-b bg-card px-3",
        className
      )}
      {...props}
    />
  )
}

function TabsTrigger({ className, ...props }: TabsPrimitive.Tab.Props) {
  return (
    <TabsPrimitive.Tab
      data-slot="tabs-trigger"
      className={cn(
        // Disabled tabs keep pointer events so their title explains why.
        "-mb-px cursor-pointer border-b-2 border-transparent px-3 pt-2.5 pb-[9px] font-medium whitespace-nowrap text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring data-active:border-primary data-active:text-foreground aria-disabled:cursor-default aria-disabled:text-faint aria-disabled:hover:text-faint",
        className
      )}
      {...props}
    />
  )
}

function TabsContent({ className, ...props }: TabsPrimitive.Panel.Props) {
  return (
    <TabsPrimitive.Panel
      data-slot="tabs-content"
      className={cn("outline-none", className)}
      {...props}
    />
  )
}

export { Tabs, TabsList, TabsTrigger, TabsContent }
